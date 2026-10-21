//! Native single-line name input, including platform text composition.

use std::ops::Range;

use gpui::{
    Bounds, ClipboardItem, Context, ElementInputHandler, EntityInputHandler, EventEmitter,
    FocusHandle, MouseButton, Pixels, Point, ShapedLine, Subscription, TextRun, UTF16Selection,
    Window, canvas, div, fill, point, prelude::*, px, rgb, size,
};
use unicode_segmentation::UnicodeSegmentation;

pub(super) enum NameEvent {
    Save,
    Cancel,
}

#[derive(Default)]
struct Text {
    value: String,
    anchor: usize,
    cursor: usize,
    marked: Option<Range<usize>>,
}

impl Text {
    fn selection(&self) -> Range<usize> {
        self.anchor.min(self.cursor)..self.anchor.max(self.cursor)
    }

    fn utf16(&self, range: Range<usize>) -> Range<usize> {
        self.value[..range.start].encode_utf16().count()
            ..self.value[..range.end].encode_utf16().count()
    }

    fn bytes(&self, range: Range<usize>) -> Range<usize> {
        from_utf16(&self.value, range.start)..from_utf16(&self.value, range.end)
    }

    fn replace(&mut self, range: Option<Range<usize>>, text: &str, marked: bool) -> Range<usize> {
        let range = range
            .map(|range| self.bytes(range))
            .or_else(|| self.marked.clone())
            .unwrap_or_else(|| self.selection());
        let remaining =
            self.value[..range.start].chars().count() + self.value[range.end..].chars().count();
        let inserted: String = text
            .chars()
            .filter(|ch| !ch.is_control())
            .take(80usize.saturating_sub(remaining))
            .collect();
        let end = range.start + inserted.len();
        self.value.replace_range(range.clone(), &inserted);
        self.anchor = end;
        self.cursor = end;
        self.marked = (marked && end > range.start).then_some(range.start..end);
        range.start..end
    }

    fn compose(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        selection: Option<Range<usize>>,
    ) {
        let inserted = self.replace(range, text, true);
        if let Some(selection) = selection {
            let value = &self.value[inserted.clone()];
            self.anchor = inserted.start + from_utf16(value, selection.start);
            self.cursor = inserted.start + from_utf16(value, selection.end);
        }
    }

    fn boundary(&self, right: bool) -> usize {
        if right {
            self.value
                .grapheme_indices(true)
                .map(|(i, _)| i)
                .find(|i| *i > self.cursor)
                .unwrap_or(self.value.len())
        } else {
            self.value
                .grapheme_indices(true)
                .map(|(i, _)| i)
                .rev()
                .find(|i| *i < self.cursor)
                .unwrap_or(0)
        }
    }
}

fn from_utf16(text: &str, offset: usize) -> usize {
    let mut units = 0;
    for (byte, ch) in text.char_indices() {
        if units >= offset {
            return byte;
        }
        units += ch.len_utf16();
    }
    text.len()
}

pub(super) struct NameField {
    text: Text,
    pub focus: FocusHandle,
    line: Option<ShapedLine>,
    bounds: Option<Bounds<Pixels>>,
    scroll: Pixels,
    dragging: bool,
    _blur: Subscription,
}

impl EventEmitter<NameEvent> for NameField {}

impl NameField {
    pub fn new(value: &str, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        let blur = cx.on_blur(&focus, window, |_, _, cx| cx.emit(NameEvent::Cancel));
        window.focus(&focus, cx);
        Self {
            text: Text {
                value: value.into(),
                cursor: value.len(),
                ..Text::default()
            },
            focus,
            line: None,
            bounds: None,
            scroll: px(0.),
            dragging: false,
            _blur: blur,
        }
    }

    pub fn value(&self) -> &str {
        &self.text.value
    }

    fn hit(&self, position: Point<Pixels>) -> usize {
        match (&self.line, self.bounds) {
            (Some(line), Some(bounds)) if line.text.as_ref() == self.text.value => {
                line.closest_index_for_x(position.x - bounds.left() + self.scroll)
            }
            _ => self.text.cursor,
        }
    }

    fn key(&mut self, event: &gpui::KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        // The platform input method owns navigation and confirmation while composing.
        if self.text.marked.is_some() {
            return;
        }
        let key = &event.keystroke;
        let mods = key.modifiers;
        let command = if cfg!(target_os = "macos") {
            mods.platform
        } else {
            mods.control
        };
        let range = self.text.selection();
        match key.key.as_str() {
            "escape" => cx.emit(NameEvent::Cancel),
            "enter" => cx.emit(NameEvent::Save),
            "u" if mods.control && !mods.alt && !mods.platform => {
                self.text.anchor = 0;
                self.text.cursor = self.text.value.len();
                self.text.replace(None, "", false);
            }
            "a" if command => {
                self.text.anchor = 0;
                self.text.cursor = self.text.value.len();
            }
            "c" | "x" if command => {
                if !range.is_empty() {
                    cx.write_to_clipboard(ClipboardItem::new_string(self.text.value[range].into()));
                    if key.key == "x" {
                        self.text.replace(None, "", false);
                    }
                }
            }
            "v" if command => {
                if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                    self.text.replace(None, &text, false);
                }
            }
            "left" | "right" | "home" | "end" if !mods.alt && !mods.control && !mods.platform => {
                self.text.cursor = match key.key.as_str() {
                    "home" => 0,
                    "end" => self.text.value.len(),
                    "left" if !mods.shift && !range.is_empty() => range.start,
                    "right" if !mods.shift && !range.is_empty() => range.end,
                    _ => self.text.boundary(key.key == "right"),
                };
                if !mods.shift {
                    self.text.anchor = self.text.cursor;
                }
            }
            "backspace" | "delete" if !mods.alt && !mods.control && !mods.platform => {
                if range.is_empty() {
                    self.text.anchor = self.text.boundary(key.key == "delete");
                }
                self.text.replace(None, "", false);
            }
            _ => return,
        }
        window.prevent_default();
        cx.stop_propagation();
        cx.notify();
    }

    fn paint(&mut self, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let style = window.text_style();
        let run = TextRun {
            len: self.text.value.len(),
            font: style.font(),
            color: style.color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let line = window.text_system().shape_line(
            self.text.value.clone().into(),
            style.font_size.to_pixels(window.rem_size()),
            &[run],
            None,
        );
        let caret = line.x_for_index(self.text.cursor);
        let width = (bounds.size.width - px(2.)).max(px(1.));
        self.scroll = self.scroll.min(caret).max(caret - width).max(px(0.));
        let origin = point(bounds.left() - self.scroll, bounds.top());
        let range = self.text.selection();
        window.with_content_mask(Some(gpui::ContentMask { bounds }), |window| {
            if !range.is_empty() {
                window.paint_quad(fill(
                    Bounds::from_corners(
                        point(origin.x + line.x_for_index(range.start), bounds.top()),
                        point(origin.x + line.x_for_index(range.end), bounds.bottom()),
                    ),
                    rgb(0x365b85),
                ));
            }
            let _ = line.paint(
                origin,
                bounds.size.height,
                gpui::TextAlign::Left,
                None,
                window,
                cx,
            );
            if let Some(marked) = &self.text.marked {
                window.paint_quad(fill(
                    Bounds::from_corners(
                        point(
                            origin.x + line.x_for_index(marked.start),
                            bounds.bottom() - px(1.),
                        ),
                        point(origin.x + line.x_for_index(marked.end), bounds.bottom()),
                    ),
                    rgb(0xe2e9f3),
                ));
            }
            if range.is_empty() && self.focus.is_focused(window) {
                window.paint_quad(fill(
                    Bounds::new(
                        point(origin.x + caret, bounds.top()),
                        size(px(1.), bounds.size.height),
                    ),
                    rgb(0xe2e9f3),
                ));
            }
        });
        self.line = Some(line);
        self.bounds = Some(bounds);
    }
}

impl EntityInputHandler for NameField {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        actual: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.text.bytes(range);
        *actual = Some(self.text.utf16(range.clone()));
        Some(self.text.value[range].into())
    }
    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.text.utf16(self.text.selection()),
            reversed: self.text.cursor < self.text.anchor,
        })
    }
    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.text.marked.clone().map(|range| self.text.utf16(range))
    }
    fn unmark_text(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.text.marked = None;
        cx.notify();
    }
    fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.text.replace(range, text, false);
        cx.notify();
    }
    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        selection: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.text.compose(range, text, selection);
        cx.notify();
    }
    fn bounds_for_range(
        &mut self,
        range: Range<usize>,
        bounds: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let line = self.line.as_ref()?;
        if line.text.as_ref() != self.text.value {
            return None;
        }
        let range = self.text.bytes(range);
        Some(Bounds::from_corners(
            point(
                bounds.left() - self.scroll + line.x_for_index(range.start),
                bounds.top(),
            ),
            point(
                bounds.left() - self.scroll + line.x_for_index(range.end),
                bounds.bottom(),
            ),
        ))
    }
    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        self.bounds?;
        Some(self.text.value[..self.hit(point)].encode_utf16().count())
    }
}

impl Render for NameField {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let entity = cx.entity();
        div()
            .w_full()
            .min_w_0()
            .h(px(24.))
            .bg(rgb(0x101620))
            .border_1()
            .border_color(rgb(0x85aaff))
            .id("name-field")
            .on_click(cx.listener(|_, _, _, cx| cx.stop_propagation()))
            .track_focus(&self.focus)
            .cursor_text()
            .on_key_down(cx.listener(Self::key))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|field, event: &gpui::MouseDownEvent, window, cx| {
                    window.focus(&field.focus, cx);
                    field.text.cursor = field.hit(event.position);
                    if !event.modifiers.shift {
                        field.text.anchor = field.text.cursor;
                    }
                    field.dragging = true;
                    cx.stop_propagation();
                    cx.notify();
                }),
            )
            .on_mouse_move(cx.listener(|field, event: &gpui::MouseMoveEvent, _, cx| {
                if field.dragging {
                    field.text.cursor = field.hit(event.position);
                    cx.notify();
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|field, _, _, _| field.dragging = false),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|field, _, _, _| field.dragging = false),
            )
            .child(
                canvas(
                    |_, _, _| (),
                    move |bounds, _, window, cx| {
                        let focus = entity.read(cx).focus.clone();
                        window.handle_input(
                            &focus,
                            ElementInputHandler::new(bounds, entity.clone()),
                            cx,
                        );
                        entity.update(cx, |field, cx| field.paint(bounds, window, cx));
                    },
                )
                .size_full(),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn composition_selection_is_relative_to_inserted_text_in_utf16() {
        let mut text = Text {
            value: "prefixZ".into(),
            anchor: 6,
            cursor: 7,
            marked: None,
        };
        text.compose(None, "😀語", Some(2..3));
        assert_eq!(text.value, "prefix😀語");
        assert_eq!(text.selection(), 10..13);
        assert_eq!(text.utf16(text.selection()), 8..9);
        text.compose(None, "日本", Some(1..1));
        assert_eq!(text.value, "prefix日本");
        assert_eq!(text.cursor, 9);
        text.replace(None, "日", false);
        assert_eq!(text.value, "prefix日");
        assert_eq!(text.cursor, text.value.len());
        assert!(text.marked.is_none());
    }

    #[test]
    fn editing_preserves_unicode_boundaries_and_limits_replacements() {
        let mut text = Text {
            value: "A😀e\u{301}Z".into(),
            cursor: 8,
            ..Text::default()
        };
        assert_eq!(text.boundary(false), 5);
        assert_eq!(text.bytes(1..3), 1..5);
        text.anchor = 1;
        text.cursor = 5;
        text.replace(None, "語", true);
        assert_eq!(text.value, "A語e\u{301}Z");
        text.replace(None, "日本", false);
        assert_eq!(text.value, "A日本e\u{301}Z");
        assert!(text.marked.is_none());
        text.anchor = 0;
        text.cursor = text.value.len();
        text.replace(None, &"😀".repeat(90), false);
        assert_eq!(text.value.chars().count(), 80);
        assert_eq!(text.cursor, 320);
        text.anchor = 0;
        text.cursor = 4;
        text.replace(None, "ab", false);
        assert!(text.value.starts_with("a😀"));
        assert_eq!(text.value.chars().count(), 80);
    }
}
