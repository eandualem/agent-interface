//! The system-webview boundary. No terminal, layout, storage or Backbone access.

use gpui::{Bounds, Pixels, Window};

#[cfg(target_os = "linux")]
thread_local! {
    static LIVE_VIEWS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
struct NativeView(wry::WebView);

#[cfg(any(target_os = "macos", target_os = "linux"))]
impl NativeView {
    fn new(view: wry::WebView) -> Self {
        #[cfg(target_os = "linux")]
        LIVE_VIEWS.with(|count| count.set(count.get() + 1));
        Self(view)
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
impl std::ops::Deref for NativeView {
    type Target = wry::WebView;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[cfg(target_os = "linux")]
impl Drop for NativeView {
    fn drop(&mut self) {
        LIVE_VIEWS.with(|count| count.set(count.get() - 1));
    }
}

#[derive(Default)]
pub(super) struct Surface {
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    view: Option<NativeView>,
    bounds: Option<Bounds<Pixels>>,
    /// The interface size the page was last zoomed to.
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    zoom: f32,
    theme: String,
    occluded: bool,
}

impl Surface {
    pub fn focus(&self) {
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        if !self.occluded
            && let Some(view) = &self.view
        {
            let _ = view.focus();
        }
    }

    pub fn focus_parent(&self) {
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        if let Some(view) = &self.view {
            let _ = view.focus_parent();
        }
    }

    pub fn clear(&mut self) {
        self.focus_parent();
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        {
            self.view = None;
        }
        self.bounds = None;
        self.theme.clear();
    }

    pub fn occlude(&mut self, occluded: bool) {
        if self.occluded != occluded {
            self.occluded = occluded;
            if occluded {
                self.focus_parent();
            }
            #[cfg(any(target_os = "macos", target_os = "linux"))]
            if let Some(view) = &self.view {
                let _ = view.set_visible(!occluded);
            }
        }
    }

    pub fn paint(
        &mut self,
        bounds: Bounds<Pixels>,
        window: &Window,
        cx: &gpui::App,
        html: &str,
        theme: &str,
        on_focus: impl Fn() + 'static,
    ) -> Result<(), String> {
        #[cfg(not(target_os = "linux"))]
        let _ = cx;
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        {
            #[cfg(target_os = "macos")]
            use wry::WebViewBuilderExtMacos;
            use wry::{
                Rect, WebViewBuilder,
                dpi::{LogicalPosition, LogicalSize},
            };
            // The child view sits outside GPUI, so it takes native
            // coordinates and zooms its page to the interface size.
            let zoom = crate::desktop::zoom::factor();
            let bounds = Bounds::new(
                gpui::point(bounds.origin.x * zoom, bounds.origin.y * zoom),
                gpui::size(bounds.size.width * zoom, bounds.size.height * zoom),
            );
            let rect = Rect {
                position: LogicalPosition::new(
                    f64::from(f32::from(bounds.origin.x)),
                    f64::from(f32::from(bounds.origin.y)),
                )
                .into(),
                size: LogicalSize::new(
                    f64::from(f32::from(bounds.size.width)),
                    f64::from(f32::from(bounds.size.height)),
                )
                .into(),
            };
            if let Some(view) = &self.view {
                if self.bounds != Some(bounds) {
                    view.set_bounds(rect).map_err(|error| error.to_string())?;
                }
                if self.zoom != zoom {
                    view.zoom(f64::from(zoom))
                        .map_err(|error| error.to_string())?;
                }
                if self.theme != theme {
                    view.set_background_color(background())
                        .map_err(|error| error.to_string())?;
                    view.evaluate_script(theme)
                        .map_err(|error| error.to_string())?;
                }
            } else if !self.occluded
                && bounds.size.width > gpui::px(0.)
                && bounds.size.height > gpui::px(0.)
            {
                let body = html.as_bytes().to_vec();
                #[cfg(target_os = "linux")]
                let on_key = {
                    let handle = window.window_handle();
                    let context = std::cell::RefCell::new(cx.to_async());
                    move |key| {
                        handle
                            .update(&mut *context.borrow_mut(), |_, window, cx| {
                                window.dispatch_keystroke(key, cx)
                            })
                            .unwrap_or(false)
                    }
                };
                #[cfg(target_os = "linux")]
                let host = agent_interface_webview_host::XlibWindow::new(window)
                    .map_err(|error| error.to_string())?;
                #[cfg(target_os = "linux")]
                let window = &host;
                let builder = WebViewBuilder::new();
                #[cfg(target_os = "macos")]
                let builder =
                    builder.with_webview_configuration(gpui_macos::artifact_configuration()?);
                let view = builder
                    .with_bounds(rect).with_focused(false).with_incognito(true)
                    .with_background_color(background())
                    .with_custom_protocol("artifact".into(), move |_, request| {
                        let valid = request.uri().host() == Some("document") && request.uri().path() == "/index.html";
                        wry::http::Response::builder().status(if valid { 200 } else { 404 })
                            .header("Content-Type", "text/html; charset=utf-8")
                            .header("Content-Security-Policy", "default-src 'none'; script-src 'unsafe-inline' 'unsafe-eval' data: blob:; style-src 'unsafe-inline' data:; img-src data: blob:; font-src data:; media-src data: blob:; worker-src blob:; connect-src 'none'; frame-src 'none'; object-src 'none'; base-uri 'none'; form-action 'none'")
                            .body(if valid { body.clone() } else { Vec::new() }.into()).expect("static response headers")
                    })
                    .with_devtools(false).with_clipboard(false)
                    .with_navigation_handler(|url| url == "artifact://document/index.html" || url.starts_with("artifact://document/index.html#"))
                    .with_new_window_req_handler(|_, _| wry::NewWindowResponse::Deny)
                    .with_permission_handler(|_| wry::PermissionResponse::Deny)
                    .with_download_started_handler(|_, _| false)
                    // The only bridge is a focus notification, never a command channel.
                    .with_initialization_script("document.addEventListener('pointerdown',()=>window.ipc.postMessage('focus'),true);")
                    .with_ipc_handler(move |request| { if request.body() == "focus" { on_focus(); } })
                    .with_initialization_script(theme)
                    .build_as_child(window).map_err(|error| error.to_string())?;
                #[cfg(target_os = "linux")]
                configure_linux(&view, on_key)?;
                // Load authored code only after native networking/file permissions
                // have been restricted, on both platforms.
                view.load_url("artifact://document/index.html")
                    .map_err(|error| error.to_string())?;
                view.zoom(f64::from(zoom))
                    .map_err(|error| error.to_string())?;
                self.view = Some(NativeView::new(view));
            }
            self.bounds = Some(bounds);
            self.zoom = zoom;
            self.theme = theme.into();
            Ok(())
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        {
            let _ = (bounds, window, html, theme, on_focus);
            Err("HTML rendering is unavailable on this platform.".into())
        }
    }
}

/// The web surface starts with the same reading background as native readers.
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn background() -> wry::RGBA {
    let color = crate::desktop::design::colors().surface;
    (
        (color.r * 255.).round() as u8,
        (color.g * 255.).round() as u8,
        (color.b * 255.).round() as u8,
        255,
    )
}

/// The host owns document text and surface colours; authored layout and media remain intact.
pub(super) fn theme() -> String {
    let c = crate::desktop::design::colors();
    let color = |value: gpui::Rgba| {
        format!(
            "#{:02x}{:02x}{:02x}",
            (value.r * 255.).round() as u8,
            (value.g * 255.).round() as u8,
            (value.b * 255.).round() as u8
        )
    };
    let scheme = if crate::desktop::design::is_light() {
        "light"
    } else {
        "dark"
    };
    let css = format!(
        r#":root{{--ai-background:{}!important;--ai-surface:{}!important;--ai-text:{}!important;--ai-muted:{}!important;--ai-accent:{}!important;--ai-line:{}!important;color-scheme:{scheme}!important;}}
html,body{{background:var(--ai-background)!important;color:var(--ai-text)!important;font-family:system-ui;-webkit-text-fill-color:currentColor!important;}}
body :not(svg,svg *,img,picture,video,canvas){{background-color:transparent!important;background-image:none!important;color:var(--ai-text)!important;border-color:var(--ai-line)!important;text-shadow:none!important;-webkit-text-fill-color:currentColor!important;-webkit-user-select:text!important;user-select:text!important;}}
body :is(pre,code,kbd,th,input,textarea,select,button){{background-color:var(--ai-surface)!important;}}
body :is(a,a *){{color:var(--ai-accent)!important;}}
body ::selection{{background:var(--ai-accent)!important;color:{}!important;}}"#,
        color(c.surface),
        color(c.selected),
        color(c.text),
        color(c.muted),
        color(c.accent),
        color(c.line),
        color(c.on_accent)
    );
    format!(
        "(()=>{{const apply=()=>{{let s=document.getElementById('agent-interface-theme');if(!s){{s=document.createElement('style');s.id='agent-interface-theme';}}s.textContent={};document.head.append(s);}};if(document.head)apply();else document.addEventListener('DOMContentLoaded',apply,{{once:true}});}})();",
        serde_json::to_string(&css).expect("CSS string")
    )
}

/// GTK shares the main thread with GPUI, but never owns the application loop.
#[cfg(target_os = "linux")]
pub(in crate::desktop) fn initialize_linux() -> Result<(), String> {
    gtk::gdk::set_allowed_backends("x11");
    gtk::init().map_err(|error| format!("Document rendering needs X11 or XWayland: {error}"))
}

#[cfg(target_os = "linux")]
pub(in crate::desktop) fn pump_linux(cx: &mut gpui::App) {
    let executor = cx.background_executor().clone();
    cx.spawn(async move |_| {
        loop {
            let delay = LIVE_VIEWS.with(|count| if count.get() == 0 { 250 } else { 16 });
            executor
                .timer(std::time::Duration::from_millis(delay))
                .await;
            // Bound each dispatch so a busy page cannot starve GPUI. Dispatch
            // outside App::update: focus notifications may update a GPUI entity.
            let started = std::time::Instant::now();
            for _ in 0..64 {
                if !gtk::events_pending()
                    || started.elapsed() >= std::time::Duration::from_millis(2)
                {
                    break;
                }
                gtk::main_iteration_do(false);
            }
        }
    })
    .detach();
}

#[cfg(target_os = "linux")]
fn configure_linux(
    view: &wry::WebView,
    on_key: impl Fn(gpui::Keystroke) -> bool + 'static,
) -> Result<(), String> {
    use gtk::prelude::WidgetExt;
    use webkit2gtk::{FileChooserRequestExt, SettingsExt, WebViewExt};
    use wry::WebViewExtUnix;
    let native = view.webview();
    let settings = WebViewExt::settings(&native).ok_or("WebKitGTK settings are unavailable")?;
    // CSP does not cover WebRTC sockets. Disable them natively, before load_url.
    settings.set_enable_webrtc(false);
    if settings.enables_webrtc() {
        return Err("WebKitGTK could not disable document peer connections".into());
    }
    settings.set_enable_dns_prefetching(false);
    settings.set_enable_hyperlink_auditing(false);
    settings.set_allow_file_access_from_file_urls(false);
    settings.set_allow_universal_access_from_file_urls(false);
    settings.set_javascript_can_access_clipboard(false);
    settings.set_javascript_can_open_windows_automatically(false);
    // Native key events may invoke configured app shortcuts. This is separate
    // from the page IPC, which accepts focus notifications only. Ordinary page
    // typing, text selection and copy/paste stay with WebKit.
    native.connect_key_press_event(move |_, event| {
        let Some(key) = linux_shortcut(event) else {
            return gtk::glib::Propagation::Proceed;
        };
        if on_key(key) {
            gtk::glib::Propagation::Stop
        } else {
            gtk::glib::Propagation::Proceed
        }
    });
    native.connect_run_file_chooser(|_, request| {
        request.cancel();
        true
    });
    // WebKit's software path works in Xvfb and WSL without a GPU. The main
    // application's native terminal renderer retains its existing GPU policy.
    if std::env::var_os("LIBGL_ALWAYS_SOFTWARE").is_some_and(|value| value == "1") {
        settings.set_hardware_acceleration_policy(webkit2gtk::HardwareAccelerationPolicy::Never);
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn linux_shortcut(event: &gtk::gdk::EventKey) -> Option<gpui::Keystroke> {
    use gtk::gdk::{Keymap, ModifierType as M, keys::Key};
    let state = event.state();
    let keyval = Keymap::default()
        .and_then(|map| {
            map.translate_keyboard_state(
                event.hardware_keycode().into(),
                state & !M::SHIFT_MASK,
                event.group().into(),
            )
        })
        .map_or(event.keyval(), |(key, ..)| Key::from(key));
    let key = match keyval.name()?.as_str() {
        "Return" | "KP_Enter" => "enter".into(),
        "Tab" | "ISO_Left_Tab" => "tab".into(),
        "Escape" => "escape".into(),
        "Left" => "left".into(),
        "Right" => "right".into(),
        "Up" => "up".into(),
        "Down" => "down".into(),
        _ => keyval.to_unicode()?.to_lowercase().to_string(),
    };
    let key = gpui::Keystroke {
        modifiers: gpui::Modifiers {
            control: state.contains(M::CONTROL_MASK),
            alt: state.contains(M::MOD1_MASK),
            shift: state.contains(M::SHIFT_MASK),
            platform: state.intersects(M::SUPER_MASK | M::META_MASK),
            ..Default::default()
        },
        key,
        key_char: None,
    };
    crate::desktop::shortcuts::command_for(&key).map(|_| key)
}
