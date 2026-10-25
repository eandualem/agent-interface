//! The system-webview boundary. No terminal, layout, storage or Backbone access.

use gpui::{Bounds, Pixels, Window};

#[derive(Default)]
pub(super) struct Surface {
    #[cfg(target_os = "macos")]
    view: Option<wry::WebView>,
    bounds: Option<Bounds<Pixels>>,
    theme: String,
    occluded: bool,
}

impl Surface {
    pub fn focus_parent(&self) {
        #[cfg(target_os = "macos")]
        if let Some(view) = &self.view {
            let _ = view.focus_parent();
        }
    }

    pub fn clear(&mut self) {
        self.focus_parent();
        #[cfg(target_os = "macos")]
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
            #[cfg(target_os = "macos")]
            if let Some(view) = &self.view {
                let _ = view.set_visible(!occluded);
            }
        }
    }

    pub fn paint(
        &mut self,
        bounds: Bounds<Pixels>,
        window: &Window,
        html: &str,
        theme: &str,
        on_focus: impl Fn() + 'static,
    ) -> Result<(), String> {
        #[cfg(target_os = "macos")]
        {
            use wry::{
                Rect, WebViewBuilder,
                dpi::{LogicalPosition, LogicalSize},
            };
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
                let view = WebViewBuilder::new()
                    .with_bounds(rect).with_focused(false).with_incognito(true)
                    .with_background_color(background())
                    .with_custom_protocol("artifact".into(), move |_, request| {
                        let valid = request.uri().host() == Some("document") && request.uri().path() == "/index.html";
                        wry::http::Response::builder().status(if valid { 200 } else { 404 })
                            .header("Content-Type", "text/html; charset=utf-8")
                            .header("Content-Security-Policy", "default-src 'none'; script-src 'unsafe-inline' 'unsafe-eval' data: blob:; style-src 'unsafe-inline' data:; img-src data: blob:; font-src data:; media-src data: blob:; worker-src blob:; connect-src 'none'; frame-src 'none'; object-src 'none'; base-uri 'none'; form-action 'none'")
                            .body(if valid { body.clone() } else { Vec::new() }.into()).expect("static response headers")
                    })
                    .with_url("artifact://document/index.html")
                    .with_navigation_handler(|url| url == "artifact://document/index.html" || url.starts_with("artifact://document/index.html#"))
                    .with_new_window_req_handler(|_, _| wry::NewWindowResponse::Deny)
                    .with_permission_handler(|_| wry::PermissionResponse::Deny)
                    .with_download_started_handler(|_, _| false)
                    // The only bridge is a focus notification, never a command channel.
                    .with_initialization_script("document.addEventListener('pointerdown',()=>window.ipc.postMessage('focus'),true);")
                    .with_ipc_handler(move |request| { if request.body() == "focus" { on_focus(); } })
                    .with_initialization_script(theme)
                    .build_as_child(window).map_err(|error| error.to_string())?;
                self.view = Some(view);
            }
            self.bounds = Some(bounds);
            self.theme = theme.into();
            Ok(())
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (bounds, window, html, theme, on_focus);
            Err("HTML rendering is available on macOS in this experiment.".into())
        }
    }
}

/// The web surface starts with the same reading background as native readers.
#[cfg(target_os = "macos")]
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
