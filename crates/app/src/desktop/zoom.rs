//! Interface size: draw every window larger or smaller than the display's
//! own scale, the way a browser zooms a page.
//!
//! GPUI takes its scale factor only from the platform, and some displays
//! report one that leaves the interface far too small or large; WSLg, for
//! one, rounds Windows' 150% down to 1. This platform wrapper multiplies the
//! window's reported scale by the chosen size and converts every logical
//! coordinate crossing the boundary, so the views, terminals and text stay
//! sharp and in proportion without knowing about it. At 100% every value
//! passes through unchanged.

use std::{
    ffi::OsString,
    ops::Range,
    path::{Path, PathBuf},
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicU32, Ordering},
    },
};

use futures::channel::oneshot;
use gpui::{
    Action, ActivationPolicy, AnyWindowHandle, App, AppLifecyclePhase, BackgroundExecutor, Bounds,
    Capslock, ClipboardItem, ClipboardReadError, CursorStyle, Decorations, DispatchEventResult,
    DisplayId, ExternalDragPayload, FileDropEvent, ForegroundExecutor, GpuSpecs,
    GraphicalEnvironment, InputHandler, Keymap, Menu, MenuItem, Modifiers, OwnedMenu,
    PathPromptOptions, Pixels, Platform, PlatformAtlas, PlatformDisplay, PlatformGestures,
    PlatformInput, PlatformInputHandler, PlatformKeyboardLayout, PlatformKeyboardMapper,
    PlatformTextSystem, PlatformWindow, Point, PromptButton, PromptLevel, RequestFrameOptions,
    ResizeEdge, Scene, ScreenCaptureSource, ScrollDelta, Size, SystemNotification,
    SystemNotificationResponse, SystemWindowTab, Task, TextInputConfiguration,
    TextInputStateChange, ThermalState, UTF16Selection, WindowAppearance,
    WindowBackgroundAppearance, WindowBounds, WindowButtonLayout, WindowControlArea,
    WindowControls, WindowDecorations, WindowInsets, WindowParams, WindowVisibility,
    WindowingRequest, point, size,
};
use raw_window_handle::{
    DisplayHandle, HandleError, HasDisplayHandle, HasWindowHandle, WindowHandle,
};

/// Interface sizes the settings accept, in percent.
pub const SIZES: [u32; 6] = [75, 100, 125, 150, 175, 200];

static FACTOR: AtomicU32 = AtomicU32::new(0);

/// The interface size as a factor of the display's scale.
pub fn factor() -> f32 {
    match FACTOR.load(Ordering::Relaxed) {
        0 => 1.0,
        bits => f32::from_bits(bits),
    }
}

/// Draw at this size, in percent, from the next frame each window lays out.
pub fn set(percent: u32) {
    FACTOR.store((percent as f32 / 100.0).to_bits(), Ordering::Relaxed);
}

/// The platform for this machine, drawing at the interface size.
pub fn platform() -> Rc<dyn Platform> {
    Rc::new(Zoomed(gpui_platform::current_platform(false)))
}

fn shrink_point(at: Point<Pixels>) -> Point<Pixels> {
    let z = factor();
    point(at.x / z, at.y / z)
}

fn grow_point(at: Point<Pixels>) -> Point<Pixels> {
    let z = factor();
    point(at.x * z, at.y * z)
}

fn shrink_size(extent: Size<Pixels>) -> Size<Pixels> {
    let z = factor();
    size(extent.width / z, extent.height / z)
}

fn grow_size(extent: Size<Pixels>) -> Size<Pixels> {
    let z = factor();
    size(extent.width * z, extent.height * z)
}

fn shrink(bounds: Bounds<Pixels>) -> Bounds<Pixels> {
    Bounds::new(shrink_point(bounds.origin), shrink_size(bounds.size))
}

fn grow(bounds: Bounds<Pixels>) -> Bounds<Pixels> {
    Bounds::new(grow_point(bounds.origin), grow_size(bounds.size))
}

fn map_window_bounds(
    bounds: WindowBounds,
    map: fn(Bounds<Pixels>) -> Bounds<Pixels>,
) -> WindowBounds {
    match bounds {
        WindowBounds::Windowed(bounds) => WindowBounds::Windowed(map(bounds)),
        WindowBounds::Maximized(bounds) => WindowBounds::Maximized(map(bounds)),
        WindowBounds::Fullscreen(bounds) => WindowBounds::Fullscreen(map(bounds)),
    }
}

/// Bring a platform event's window coordinates into the zoomed space.
fn shrink_input(mut input: PlatformInput) -> PlatformInput {
    match &mut input {
        PlatformInput::MouseDown(event) => event.position = shrink_point(event.position),
        PlatformInput::MouseUp(event) => event.position = shrink_point(event.position),
        PlatformInput::MousePressure(event) => event.position = shrink_point(event.position),
        PlatformInput::MouseMove(event) => event.position = shrink_point(event.position),
        PlatformInput::MouseExited(event) => event.position = shrink_point(event.position),
        PlatformInput::ScrollWheel(event) => {
            event.position = shrink_point(event.position);
            if let ScrollDelta::Pixels(delta) = &mut event.delta {
                *delta = shrink_point(*delta);
            }
        }
        PlatformInput::Pinch(event) => event.position = shrink_point(event.position),
        PlatformInput::LongPress(event) => {
            event.start_position = shrink_point(event.start_position);
            event.position = shrink_point(event.position);
        }
        PlatformInput::TouchDrag(event) => {
            event.start_position = shrink_point(event.start_position);
            event.position = shrink_point(event.position);
        }
        PlatformInput::FileDrop(
            FileDropEvent::Entered { position, .. }
            | FileDropEvent::Pending { position }
            | FileDropEvent::Submit { position },
        ) => *position = shrink_point(*position),
        PlatformInput::Touch(event) => {
            event.position = shrink_point(event.position);
            event.predicted_position = event.predicted_position.map(shrink_point);
        }
        _ => {}
    }
    input
}

struct Zoomed(Rc<dyn Platform>);

impl Platform for Zoomed {
    fn background_executor(&self) -> BackgroundExecutor {
        self.0.background_executor()
    }
    fn foreground_executor(&self) -> ForegroundExecutor {
        self.0.foreground_executor()
    }
    fn text_system(&self) -> Arc<dyn PlatformTextSystem> {
        self.0.text_system()
    }
    fn run(&self, on_finish_launching: Box<dyn 'static + FnOnce()>) {
        self.0.run(on_finish_launching)
    }
    fn quit(&self) {
        self.0.quit()
    }
    fn set_initial_windowing(&self, request: WindowingRequest) {
        self.0.set_initial_windowing(request)
    }
    fn request_windowing(&self, request: WindowingRequest) -> Task<gpui::Result<()>> {
        self.0.request_windowing(request)
    }
    fn restart(&self, binary_path: Option<PathBuf>, arguments: Vec<OsString>) {
        self.0.restart(binary_path, arguments)
    }
    fn activate(&self, ignoring_other_apps: bool) {
        self.0.activate(ignoring_other_apps)
    }
    fn set_activation_policy(&self, policy: ActivationPolicy) {
        self.0.set_activation_policy(policy)
    }
    fn hide(&self) {
        self.0.hide()
    }
    fn hide_other_apps(&self) {
        self.0.hide_other_apps()
    }
    fn unhide_other_apps(&self) {
        self.0.unhide_other_apps()
    }
    fn displays(&self) -> Vec<Rc<dyn PlatformDisplay>> {
        self.0.displays().into_iter().map(display).collect()
    }
    fn primary_display(&self) -> Option<Rc<dyn PlatformDisplay>> {
        self.0.primary_display().map(display)
    }
    fn on_displays_changed(&self, callback: Box<dyn FnMut()>) {
        self.0.on_displays_changed(callback)
    }
    fn active_window(&self) -> Option<AnyWindowHandle> {
        self.0.active_window()
    }
    fn window_stack(&self) -> Option<Vec<AnyWindowHandle>> {
        self.0.window_stack()
    }
    fn is_screen_capture_supported(&self) -> bool {
        self.0.is_screen_capture_supported()
    }
    fn screen_capture_sources(
        &self,
    ) -> oneshot::Receiver<gpui::Result<Vec<Rc<dyn ScreenCaptureSource>>>> {
        self.0.screen_capture_sources()
    }
    fn open_window(
        &self,
        handle: AnyWindowHandle,
        mut options: WindowParams,
    ) -> gpui::Result<Box<dyn PlatformWindow>> {
        options.bounds = grow(options.bounds);
        options.window_min_size = options.window_min_size.map(grow_size);
        if let Some(titlebar) = &mut options.titlebar {
            titlebar.traffic_light_position = titlebar.traffic_light_position.map(grow_point);
        }
        Ok(Box::new(Window(self.0.open_window(handle, options)?)))
    }
    fn window_appearance(&self) -> WindowAppearance {
        self.0.window_appearance()
    }
    fn set_window_appearance(&self, appearance: Option<WindowAppearance>) {
        self.0.set_window_appearance(appearance)
    }
    fn button_layout(&self) -> Option<WindowButtonLayout> {
        self.0.button_layout()
    }
    fn open_url(&self, url: &str) {
        self.0.open_url(url)
    }
    fn on_open_urls(&self, callback: Box<dyn FnMut(Vec<String>)>) {
        self.0.on_open_urls(callback)
    }
    fn register_url_scheme(&self, url: &str) -> Task<gpui::Result<()>> {
        self.0.register_url_scheme(url)
    }
    fn prompt_for_paths(
        &self,
        options: PathPromptOptions,
    ) -> oneshot::Receiver<gpui::Result<Option<Vec<PathBuf>>>> {
        self.0.prompt_for_paths(options)
    }
    fn prompt_for_new_path(
        &self,
        directory: &Path,
        suggested_name: Option<&str>,
    ) -> oneshot::Receiver<gpui::Result<Option<PathBuf>>> {
        self.0.prompt_for_new_path(directory, suggested_name)
    }
    fn can_select_mixed_files_and_dirs(&self) -> bool {
        self.0.can_select_mixed_files_and_dirs()
    }
    fn reveal_path(&self, path: &Path) {
        self.0.reveal_path(path)
    }
    fn open_with_system(&self, path: &Path) {
        self.0.open_with_system(path)
    }
    fn on_quit(&self, callback: Box<dyn FnMut() -> bool>) {
        self.0.on_quit(callback)
    }
    fn on_reopen(&self, callback: Box<dyn FnMut()>) {
        self.0.on_reopen(callback)
    }
    fn on_system_sleep(&self, callback: Box<dyn FnMut()>) {
        self.0.on_system_sleep(callback)
    }
    fn on_system_wake(&self, callback: Box<dyn FnMut()>) {
        self.0.on_system_wake(callback)
    }
    fn on_app_lifecycle(&self, callback: Box<dyn FnMut(AppLifecyclePhase)>) {
        self.0.on_app_lifecycle(callback)
    }
    fn on_memory_warning(&self, callback: Box<dyn FnMut()>) {
        self.0.on_memory_warning(callback)
    }
    fn gestures(&self) -> Option<Rc<dyn PlatformGestures>> {
        self.0.gestures()
    }
    fn set_menus(&self, menus: Vec<Menu>, keymap: &Keymap) {
        self.0.set_menus(menus, keymap)
    }
    fn get_menus(&self) -> Option<Vec<OwnedMenu>> {
        self.0.get_menus()
    }
    fn set_dock_menu(&self, menu: Vec<MenuItem>, keymap: &Keymap) {
        self.0.set_dock_menu(menu, keymap)
    }
    fn perform_dock_menu_action(&self, action: usize) {
        self.0.perform_dock_menu_action(action)
    }
    fn add_recent_document(&self, path: &Path) {
        self.0.add_recent_document(path)
    }
    // `update_jump_list` keeps its default: only native Windows implements
    // it, and this app has no native Windows build.
    fn on_app_menu_action(&self, callback: Box<dyn FnMut(&dyn Action)>) {
        self.0.on_app_menu_action(callback)
    }
    fn on_will_open_app_menu(&self, callback: Box<dyn FnMut()>) {
        self.0.on_will_open_app_menu(callback)
    }
    fn on_validate_app_menu_command(&self, callback: Box<dyn FnMut(&dyn Action) -> bool>) {
        self.0.on_validate_app_menu_command(callback)
    }
    fn thermal_state(&self) -> ThermalState {
        self.0.thermal_state()
    }
    fn on_thermal_state_change(&self, callback: Box<dyn FnMut()>) {
        self.0.on_thermal_state_change(callback)
    }
    fn prevent_idle_sleep(&self, reason: &str) -> Task<gpui::Result<gpui::ActivityGuard>> {
        self.0.prevent_idle_sleep(reason)
    }
    fn set_app_identity(&self, identifier: &str, name: &str) {
        self.0.set_app_identity(identifier, name)
    }
    fn show_system_notification(&self, notification: SystemNotification) {
        self.0.show_system_notification(notification)
    }
    fn dismiss_system_notification(&self, tag: &str) {
        self.0.dismiss_system_notification(tag)
    }
    fn on_system_notification_response(
        &self,
        callback: Box<dyn FnMut(SystemNotificationResponse)>,
    ) {
        self.0.on_system_notification_response(callback)
    }
    fn compositor_name(&self) -> &'static str {
        self.0.compositor_name()
    }
    fn graphical_environment(&self) -> Option<GraphicalEnvironment> {
        self.0.graphical_environment()
    }
    fn app_path(&self) -> gpui::Result<PathBuf> {
        self.0.app_path()
    }
    fn path_for_auxiliary_executable(&self, name: &str) -> gpui::Result<PathBuf> {
        self.0.path_for_auxiliary_executable(name)
    }
    fn set_cursor_style(&self, style: CursorStyle) {
        self.0.set_cursor_style(style)
    }
    fn hide_cursor_until_mouse_moves(&self) {
        self.0.hide_cursor_until_mouse_moves()
    }
    fn is_cursor_visible(&self) -> bool {
        self.0.is_cursor_visible()
    }
    fn should_auto_hide_scrollbars(&self) -> bool {
        self.0.should_auto_hide_scrollbars()
    }
    fn read_from_clipboard(&self) -> Option<ClipboardItem> {
        self.0.read_from_clipboard()
    }
    fn write_to_clipboard(&self, item: ClipboardItem) {
        self.0.write_to_clipboard(item)
    }
    fn read_from_clipboard_async(&self) -> Task<Result<Option<ClipboardItem>, ClipboardReadError>> {
        self.0.read_from_clipboard_async()
    }
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    fn read_from_primary(&self) -> Option<ClipboardItem> {
        self.0.read_from_primary()
    }
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    fn write_to_primary(&self, item: ClipboardItem) {
        self.0.write_to_primary(item)
    }
    #[cfg(target_os = "macos")]
    fn read_from_find_pasteboard(&self) -> Option<ClipboardItem> {
        self.0.read_from_find_pasteboard()
    }
    #[cfg(target_os = "macos")]
    fn write_to_find_pasteboard(&self, item: ClipboardItem) {
        self.0.write_to_find_pasteboard(item)
    }
    fn write_credentials(
        &self,
        url: &str,
        username: &str,
        password: &[u8],
    ) -> Task<gpui::Result<()>> {
        self.0.write_credentials(url, username, password)
    }
    fn read_credentials(&self, url: &str) -> Task<gpui::Result<Option<(String, Vec<u8>)>>> {
        self.0.read_credentials(url)
    }
    fn delete_credentials(&self, url: &str) -> Task<gpui::Result<()>> {
        self.0.delete_credentials(url)
    }
    fn keyboard_layout(&self) -> Box<dyn PlatformKeyboardLayout> {
        self.0.keyboard_layout()
    }
    fn keyboard_mapper(&self) -> Rc<dyn PlatformKeyboardMapper> {
        self.0.keyboard_mapper()
    }
    fn on_keyboard_layout_change(&self, callback: Box<dyn FnMut()>) {
        self.0.on_keyboard_layout_change(callback)
    }
}

fn display(inner: Rc<dyn PlatformDisplay>) -> Rc<dyn PlatformDisplay> {
    Rc::new(Display(inner))
}

#[derive(Debug)]
struct Display(Rc<dyn PlatformDisplay>);

impl PlatformDisplay for Display {
    fn id(&self) -> DisplayId {
        self.0.id()
    }
    fn uuid(&self) -> gpui::Result<uuid::Uuid> {
        self.0.uuid()
    }
    fn bounds(&self) -> Bounds<Pixels> {
        shrink(self.0.bounds())
    }
    fn refresh_interval(&self) -> Option<std::time::Duration> {
        self.0.refresh_interval()
    }
    fn visible_bounds(&self) -> Bounds<Pixels> {
        shrink(self.0.visible_bounds())
    }
    fn default_bounds(&self) -> Bounds<Pixels> {
        shrink(self.0.default_bounds())
    }
}

struct Window(Box<dyn PlatformWindow>);

impl HasWindowHandle for Window {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        self.0.window_handle()
    }
}

impl HasDisplayHandle for Window {
    fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
        self.0.display_handle()
    }
}

impl PlatformWindow for Window {
    fn bounds(&self) -> Bounds<Pixels> {
        shrink(self.0.bounds())
    }
    fn is_maximized(&self) -> bool {
        self.0.is_maximized()
    }
    fn window_bounds(&self) -> WindowBounds {
        map_window_bounds(self.0.window_bounds(), shrink)
    }
    fn content_size(&self) -> Size<Pixels> {
        shrink_size(self.0.content_size())
    }
    fn visual_viewport_bounds(&self) -> Bounds<Pixels> {
        shrink(self.0.visual_viewport_bounds())
    }
    fn on_visual_viewport_changed(&self, callback: Box<dyn FnMut()>) {
        self.0.on_visual_viewport_changed(callback)
    }
    fn prepare_frame(&self) -> bool {
        self.0.prepare_frame()
    }
    fn resize(&mut self, extent: Size<Pixels>) {
        self.0.resize(grow_size(extent))
    }
    fn scale_factor(&self) -> f32 {
        self.0.scale_factor() * factor()
    }
    fn appearance(&self) -> WindowAppearance {
        self.0.appearance()
    }
    fn display(&self) -> Option<Rc<dyn PlatformDisplay>> {
        self.0.display().map(display)
    }
    fn mouse_position(&self) -> Point<Pixels> {
        shrink_point(self.0.mouse_position())
    }
    fn modifiers(&self) -> Modifiers {
        self.0.modifiers()
    }
    fn capslock(&self) -> Capslock {
        self.0.capslock()
    }
    // Handlers registered through `input` already report native geometry.
    fn set_input_handler(&mut self, input_handler: PlatformInputHandler) {
        self.0.set_input_handler(input_handler)
    }
    fn take_input_handler(&mut self) -> Option<PlatformInputHandler> {
        self.0.take_input_handler()
    }
    fn set_text_input_configuration(&mut self, configuration: TextInputConfiguration) {
        self.0.set_text_input_configuration(configuration)
    }
    fn prompt(
        &self,
        level: PromptLevel,
        msg: &str,
        detail: Option<&str>,
        answers: &[PromptButton],
    ) -> Option<oneshot::Receiver<usize>> {
        self.0.prompt(level, msg, detail, answers)
    }
    fn activate(&self) {
        self.0.activate()
    }
    fn request_attention(&self) {
        self.0.request_attention()
    }
    fn is_active(&self) -> bool {
        self.0.is_active()
    }
    fn visibility(&self) -> WindowVisibility {
        self.0.visibility()
    }
    fn is_hovered(&self) -> bool {
        self.0.is_hovered()
    }
    fn background_appearance(&self) -> WindowBackgroundAppearance {
        self.0.background_appearance()
    }
    fn set_title(&mut self, title: &str) {
        self.0.set_title(title)
    }
    fn set_background_appearance(&self, background_appearance: WindowBackgroundAppearance) {
        self.0.set_background_appearance(background_appearance)
    }
    fn minimize(&self) {
        self.0.minimize()
    }
    fn zoom(&self) {
        self.0.zoom()
    }
    fn toggle_fullscreen(&self) {
        self.0.toggle_fullscreen()
    }
    fn is_fullscreen(&self) -> bool {
        self.0.is_fullscreen()
    }
    fn frame_waker(&self) -> Option<Rc<dyn Fn()>> {
        self.0.frame_waker()
    }
    fn on_request_frame(&self, callback: Box<dyn FnMut(RequestFrameOptions)>) {
        self.0.on_request_frame(callback)
    }
    fn on_input(&self, mut callback: Box<dyn FnMut(PlatformInput) -> DispatchEventResult>) {
        self.0
            .on_input(Box::new(move |input| callback(shrink_input(input))))
    }
    fn on_active_status_change(&self, callback: Box<dyn FnMut(bool)>) {
        self.0.on_active_status_change(callback)
    }
    fn on_visibility_change(&self, callback: Box<dyn FnMut(WindowVisibility)>) {
        self.0.on_visibility_change(callback)
    }
    fn on_hover_status_change(&self, callback: Box<dyn FnMut(bool)>) {
        self.0.on_hover_status_change(callback)
    }
    fn on_resize(&self, mut callback: Box<dyn FnMut(Size<Pixels>, f32)>) {
        self.0.on_resize(Box::new(move |extent, scale| {
            callback(shrink_size(extent), scale * factor())
        }))
    }
    fn on_moved(&self, callback: Box<dyn FnMut()>) {
        self.0.on_moved(callback)
    }
    fn on_display_changed(&self, callback: Box<dyn FnMut()>) {
        self.0.on_display_changed(callback)
    }
    fn on_should_close(&self, callback: Box<dyn FnMut() -> bool>) {
        self.0.on_should_close(callback)
    }
    fn on_hit_test_window_control(&self, callback: Box<dyn FnMut() -> Option<WindowControlArea>>) {
        self.0.on_hit_test_window_control(callback)
    }
    fn on_close(&self, callback: Box<dyn FnOnce()>) {
        self.0.on_close(callback)
    }
    fn on_appearance_changed(&self, callback: Box<dyn FnMut()>) {
        self.0.on_appearance_changed(callback)
    }
    fn on_button_layout_changed(&self, callback: Box<dyn FnMut()>) {
        self.0.on_button_layout_changed(callback)
    }
    fn draw(&self, scene: &Scene) {
        self.0.draw(scene)
    }
    fn schedule_frame(&self) {
        self.0.schedule_frame()
    }
    fn sprite_atlas(&self) -> Arc<dyn PlatformAtlas> {
        self.0.sprite_atlas()
    }
    fn is_subpixel_rendering_supported(&self) -> bool {
        self.0.is_subpixel_rendering_supported()
    }
    fn get_title(&self) -> String {
        self.0.get_title()
    }
    fn tabbed_windows(&self) -> Option<Vec<SystemWindowTab>> {
        self.0.tabbed_windows()
    }
    fn tab_bar_visible(&self) -> bool {
        self.0.tab_bar_visible()
    }
    fn set_edited(&mut self, edited: bool) {
        self.0.set_edited(edited)
    }
    fn set_document_path(&self, path: Option<&Path>) {
        self.0.set_document_path(path)
    }
    fn toggle_simple_fullscreen(&self) {
        self.0.toggle_simple_fullscreen()
    }
    fn is_simple_fullscreen(&self) -> bool {
        self.0.is_simple_fullscreen()
    }
    #[cfg(target_os = "macos")]
    fn set_traffic_light_position(&self, position: Point<Pixels>) {
        self.0.set_traffic_light_position(grow_point(position))
    }
    fn show_character_palette(&self) {
        self.0.show_character_palette()
    }
    fn titlebar_double_click(&self, is_resizable: bool, is_minimizable: bool) {
        self.0.titlebar_double_click(is_resizable, is_minimizable)
    }
    fn on_move_tab_to_new_window(&self, callback: Box<dyn FnMut()>) {
        self.0.on_move_tab_to_new_window(callback)
    }
    fn on_merge_all_windows(&self, callback: Box<dyn FnMut()>) {
        self.0.on_merge_all_windows(callback)
    }
    fn on_select_previous_tab(&self, callback: Box<dyn FnMut()>) {
        self.0.on_select_previous_tab(callback)
    }
    fn on_select_next_tab(&self, callback: Box<dyn FnMut()>) {
        self.0.on_select_next_tab(callback)
    }
    fn on_toggle_tab_bar(&self, callback: Box<dyn FnMut()>) {
        self.0.on_toggle_tab_bar(callback)
    }
    fn merge_all_windows(&self) {
        self.0.merge_all_windows()
    }
    fn move_tab_to_new_window(&self) {
        self.0.move_tab_to_new_window()
    }
    fn toggle_window_tab_overview(&self) {
        self.0.toggle_window_tab_overview()
    }
    fn set_tabbing_identifier(&self, identifier: Option<String>) {
        self.0.set_tabbing_identifier(identifier)
    }
    fn native_window_state(&self) -> Option<Vec<u8>> {
        self.0.native_window_state()
    }
    fn restore_native_window_state(&self, state: &[u8]) {
        self.0.restore_native_window_state(state)
    }
    fn inner_window_bounds(&self) -> WindowBounds {
        map_window_bounds(self.0.inner_window_bounds(), shrink)
    }
    fn request_decorations(&self, decorations: WindowDecorations) {
        self.0.request_decorations(decorations)
    }
    fn show_window_menu(&self, position: Point<Pixels>) {
        self.0.show_window_menu(grow_point(position))
    }
    fn start_window_move(&self) {
        self.0.start_window_move()
    }
    fn can_start_external_drag(&self) -> bool {
        self.0.can_start_external_drag()
    }
    fn start_external_drag(&self, payload: &ExternalDragPayload) -> bool {
        self.0.start_external_drag(payload)
    }
    fn start_window_resize(&self, edge: ResizeEdge) {
        self.0.start_window_resize(edge)
    }
    fn set_exclusive_zone(&self, zone: Pixels) {
        self.0.set_exclusive_zone(zone * factor())
    }
    #[cfg(target_os = "linux")]
    fn set_exclusive_edge(&self, edge: gpui::layer_shell::Anchor) {
        self.0.set_exclusive_edge(edge)
    }
    fn set_input_region(&self, region: Option<&[Bounds<Pixels>]>) {
        let region = region.map(|region| region.iter().copied().map(grow).collect::<Vec<_>>());
        self.0.set_input_region(region.as_deref())
    }
    fn window_decorations(&self) -> Decorations {
        self.0.window_decorations()
    }
    fn set_app_id(&mut self, app_id: &str) {
        self.0.set_app_id(app_id)
    }
    fn map_window(&mut self) -> gpui::Result<()> {
        self.0.map_window()
    }
    fn window_controls(&self) -> WindowControls {
        self.0.window_controls()
    }
    fn set_client_inset(&self, inset: Pixels) {
        self.0.set_client_inset(inset * factor())
    }
    fn gpu_specs(&self) -> Option<GpuSpecs> {
        self.0.gpu_specs()
    }
    // GPUI takes these bounds from the input handler, which `input` converts.
    fn update_ime_position(&self, bounds: Bounds<Pixels>) {
        self.0.update_ime_position(bounds)
    }
    // Insets and the soft keyboard belong to mobile platforms.
    fn insets(&self) -> WindowInsets {
        self.0.insets()
    }
    fn on_insets_changed(&self, callback: Box<dyn FnMut(WindowInsets)>) {
        self.0.on_insets_changed(callback)
    }
    fn set_back_handler(&self, callback: Box<dyn FnMut()>) {
        self.0.set_back_handler(callback)
    }
    fn set_back_enabled(&self, enabled: bool) {
        self.0.set_back_enabled(enabled)
    }
    fn show_soft_keyboard(&self) {
        self.0.show_soft_keyboard()
    }
    fn hide_soft_keyboard(&self) {
        self.0.hide_soft_keyboard()
    }
    fn text_input_state_changed(&self, change: TextInputStateChange) {
        self.0.text_input_state_changed(change)
    }
    fn play_system_bell(&self) {
        self.0.play_system_bell()
    }
    fn a11y_init(&self, callbacks: gpui::A11yCallbacks) {
        self.0.a11y_init(callbacks)
    }
    fn a11y_tree_update(&self, tree_update: gpui::accesskit::TreeUpdate) {
        self.0.a11y_tree_update(tree_update)
    }
    fn a11y_update_window_bounds(&self) {
        self.0.a11y_update_window_bounds()
    }
}

/// A text input handler whose caret geometry the platform reads in its own
/// coordinates. Register every input handler through this.
pub fn input(handler: impl InputHandler) -> impl InputHandler {
    Input(handler)
}

struct Input<H>(H);

impl<H: InputHandler> InputHandler for Input<H> {
    fn selected_text_range(
        &mut self,
        ignore_disabled_input: bool,
        window: &mut gpui::Window,
        cx: &mut App,
    ) -> Option<UTF16Selection> {
        self.0
            .selected_text_range(ignore_disabled_input, window, cx)
    }
    fn marked_text_range(
        &mut self,
        window: &mut gpui::Window,
        cx: &mut App,
    ) -> Option<Range<usize>> {
        self.0.marked_text_range(window, cx)
    }
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        adjusted_range: &mut Option<Range<usize>>,
        window: &mut gpui::Window,
        cx: &mut App,
    ) -> Option<String> {
        self.0
            .text_for_range(range_utf16, adjusted_range, window, cx)
    }
    fn replace_text_in_range(
        &mut self,
        replacement_range: Option<Range<usize>>,
        text: &str,
        window: &mut gpui::Window,
        cx: &mut App,
    ) {
        self.0
            .replace_text_in_range(replacement_range, text, window, cx)
    }
    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range: Option<Range<usize>>,
        window: &mut gpui::Window,
        cx: &mut App,
    ) {
        self.0
            .replace_and_mark_text_in_range(range_utf16, new_text, new_selected_range, window, cx)
    }
    fn unmark_text(&mut self, window: &mut gpui::Window, cx: &mut App) {
        self.0.unmark_text(window, cx)
    }
    fn paste(&mut self, item: ClipboardItem, window: &mut gpui::Window, cx: &mut App) {
        self.0.paste(item, window, cx)
    }
    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        window: &mut gpui::Window,
        cx: &mut App,
    ) -> Option<Bounds<Pixels>> {
        self.0.bounds_for_range(range_utf16, window, cx).map(grow)
    }
    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        window: &mut gpui::Window,
        cx: &mut App,
    ) -> Option<usize> {
        self.0
            .character_index_for_point(shrink_point(point), window, cx)
    }
    fn set_selected_text_range(
        &mut self,
        range_utf16: Range<usize>,
        window: &mut gpui::Window,
        cx: &mut App,
    ) {
        self.0.set_selected_text_range(range_utf16, window, cx)
    }
    fn element_bounds(
        &mut self,
        window: &mut gpui::Window,
        cx: &mut App,
    ) -> Option<Bounds<Pixels>> {
        self.0.element_bounds(window, cx).map(grow)
    }
    fn text_length_utf16(&mut self, window: &mut gpui::Window, cx: &mut App) -> Option<usize> {
        self.0.text_length_utf16(window, cx)
    }
    fn apple_press_and_hold_enabled(&mut self) -> bool {
        self.0.apple_press_and_hold_enabled()
    }
    fn accepts_text_input(&mut self, window: &mut gpui::Window, cx: &mut App) -> bool {
        self.0.accepts_text_input(window, cx)
    }
    fn text_input_editable_range(
        &mut self,
        window: &mut gpui::Window,
        cx: &mut App,
    ) -> Option<Range<usize>> {
        self.0.text_input_editable_range(window, cx)
    }
    fn prefers_ime_for_printable_keys(&mut self, window: &mut gpui::Window, cx: &mut App) -> bool {
        self.0.prefers_ime_for_printable_keys(window, cx)
    }
    fn text_input_configuration(
        &mut self,
        window: &mut gpui::Window,
        cx: &mut App,
    ) -> TextInputConfiguration {
        self.0.text_input_configuration(window, cx)
    }
}
