//! Native name, filter and multiline feedback input, including composition.

use std::ops::Range;

use gpui::{
    Bounds, ClipboardItem, Context, ElementInputHandler, EntityInputHandler, EventEmitter,
    FocusHandle, MouseButton, Pixels, Point, ShapedLine, Subscription, TextRun, UTF16Selection,
    Window, canvas, div, fill, point, prelude::*, px, size,
};
use unicode_properties::{GeneralCategoryGroup, UnicodeGeneralCategory};
use unicode_segmentation::UnicodeSegmentation;

pub(super) enum NameEvent {
    Save,
    Cancel,
    Changed(String),
}

#[derive(Default)]
struct Text {
    value: String,
    anchor: usize,
    cursor: usize,
    marked: Option<Range<usize>>,
    search: bool,
    feedback: bool,
    limit: Option<usize>,
    rejected: bool,
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

    fn replace(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        marked: bool,
    ) -> Option<Range<usize>> {
        let range = range
            .map(|range| self.bytes(range))
            .or_else(|| self.marked.clone())
            .unwrap_or_else(|| self.selection());
        let remaining =
            self.value[..range.start].chars().count() + self.value[range.end..].chars().count();
        let available = self
            .limit
            .unwrap_or(if self.feedback {
                agent_interface_roster::feedback::TEXT_LIMIT
            } else {
                80
            })
            .saturating_sub(remaining);
        let inserted: String = text
            .chars()
            .filter(|ch| {
                ((!ch.is_control()) || (self.feedback && matches!(ch, '\n' | '\t')))
                    && (!self.search
                        || *ch == ' '
                        || !matches!(
                            ch.general_category_group(),
                            GeneralCategoryGroup::Other | GeneralCategoryGroup::Separator
                        ))
            })
            .take(available.saturating_add(usize::from(self.limit.is_some())))
            .collect();
        self.rejected = self.limit.is_some() && inserted.chars().count() > available;
        if self.rejected {
            return None;
        }
        let end = range.start + inserted.len();
        self.value.replace_range(range.clone(), &inserted);
        self.cursor = if marked { end } else { self.snap(end) };
        self.anchor = self.cursor;
        self.marked = (marked && end > range.start).then_some(range.start..end);
        Some(range.start..end)
    }

    fn compose(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        selection: Option<Range<usize>>,
    ) {
        let Some(inserted) = self.replace(range, text, true) else {
            return;
        };
        if let Some(selection) = selection {
            let value = &self.value[inserted.clone()];
            self.anchor = inserted.start + from_utf16(value, selection.start);
            self.cursor = inserted.start + from_utf16(value, selection.end);
        }
    }

    fn snap(&self, offset: usize) -> usize {
        self.value
            .grapheme_indices(true)
            .map(|(i, _)| i)
            .find(|i| *i >= offset)
            .unwrap_or(self.value.len())
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
    feedback_lines: Vec<gpui::WrappedLine>,
    feedback_text: String,
    line_height: Pixels,
    bounds: Option<Bounds<Pixels>>,
    scroll: Pixels,
    dragging: bool,
    cancel_on_blur: bool,
    read_only: bool,
    _blur: Subscription,
}

impl EventEmitter<NameEvent> for NameField {}

impl NameField {
    pub fn new(value: &str, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        let blur = cx.on_blur(&focus, window, |field, _, cx| {
            if field.cancel_on_blur {
                cx.emit(NameEvent::Cancel);
            }
        });
        window.focus(&focus, cx);
        Self {
            text: Text {
                value: value.into(),
                cursor: value.len(),
                ..Text::default()
            },
            focus,
            line: None,
            feedback_lines: Vec::new(),
            feedback_text: String::new(),
            line_height: px(20.),
            bounds: None,
            scroll: px(0.),
            dragging: false,
            cancel_on_blur: true,
            read_only: false,
            _blur: blur,
        }
    }

    pub fn search(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut field = Self::new("", window, cx);
        field.text.search = true;
        field.cancel_on_blur = false;
        field
    }

    /// A search field that waits for a click or an explicit focus, for panes
    /// that are built before they are chosen.
    pub fn filter(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let previous = window.focused(cx);
        let field = Self::search(window, cx);
        match previous {
            Some(previous) => window.focus(&previous, cx),
            None => window.blur(cx),
        }
        field
    }

    /// A multiline feedback field that keeps the current keyboard focus.
    pub fn feedback(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut field = Self::filter(window, cx);
        field.text.search = false;
        field.text.feedback = true;
        field
    }

    /// A bounded value editor, retaining native IME and clipboard behavior.
    pub fn value_field(
        value: &str,
        limit: usize,
        multiline: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut field = Self::filter(window, cx);
        field.text.search = false;
        field.text.feedback = multiline;
        field.text.limit = Some(limit);
        field.set_value(value, cx);
        field
    }

    /// Replace a selected record's input without reporting a user edit.
    pub fn set_value(&mut self, value: &str, cx: &mut Context<Self>) {
        self.text.value = value.into();
        self.text.anchor = 0;
        self.text.cursor = value.len();
        self.text.marked = None;
        self.text.rejected = false;
        self.scroll = px(0.);
        cx.notify();
    }

    /// Freeze input while its exact contents are being committed.
    pub fn set_read_only(&mut self, read_only: bool) {
        self.read_only = read_only;
    }

    /// Empty the field without announcing a change.
    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.text.value.clear();
        self.text.anchor = 0;
        self.text.cursor = 0;
        self.text.marked = None;
        self.text.rejected = false;
        cx.notify();
    }

    pub fn composing(&self) -> bool {
        self.text.marked.is_some()
    }

    pub fn value(&self) -> &str {
        &self.text.value
    }

    fn hit(&self, position: Point<Pixels>) -> usize {
        if self.text.feedback {
            let Some(bounds) = self
                .bounds
                .filter(|_| self.feedback_text == self.text.value)
            else {
                return self.text.cursor;
            };
            let mut point = position - bounds.origin + gpui::point(px(0.), self.scroll);
            let mut start = 0;
            for line in &self.feedback_lines {
                let height = line.size(self.line_height).height;
                if point.y < height {
                    let byte = line
                        .closest_index_for_position(point, self.line_height)
                        .unwrap_or_else(|byte| byte);
                    return self.text.snap(start + byte);
                }
                start += line.len() + 1;
                point.y -= height;
            }
            return self.text.value.len();
        }
        match (&self.line, self.bounds) {
            (Some(line), Some(bounds)) if line.text.as_ref() == self.text.value => self
                .text
                .snap(line.closest_index_for_x(position.x - bounds.left() + self.scroll)),
            _ => self.text.cursor,
        }
    }

    fn key(&mut self, event: &gpui::KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.read_only {
            cx.stop_propagation();
            window.prevent_default();
            return;
        }
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
        let before = self.text.value.clone();
        let range = self.text.selection();
        match key.key.as_str() {
            "escape" => cx.emit(NameEvent::Cancel),
            "enter" if self.text.feedback && !mods.platform && !mods.control && !mods.alt => {
                self.text.replace(None, "\n", false);
            }
            "enter" if !mods.platform && !mods.control && !mods.alt && !mods.shift => {
                cx.emit(NameEvent::Save)
            }
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
            "up" | "down" if self.text.feedback && !mods.alt && !mods.control && !mods.platform => {
                if let (Some(at), Some(bounds)) =
                    (self.feedback_position(self.text.cursor), self.bounds)
                {
                    let direction = if key.key == "up" { -1. } else { 1. };
                    self.text.cursor = self.hit(
                        bounds.origin
                            + at
                            + point(px(0.), self.line_height * direction - self.scroll),
                    );
                    if !mods.shift {
                        self.text.anchor = self.text.cursor;
                    }
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
        if self.text.value != before {
            cx.emit(NameEvent::Changed(self.text.value.clone()));
        }
        window.prevent_default();
        cx.stop_propagation();
        cx.notify();
    }

    fn paint(&mut self, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        if self.text.feedback {
            self.paint_feedback(bounds, window, cx);
            return;
        }
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
        let caret_width = px(super::design::tokens().size.caret);
        let width = (bounds.size.width - caret_width * 2.0).max(caret_width);
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
                    super::design::colors().accent_soft,
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
                            bounds.bottom() - caret_width,
                        ),
                        point(origin.x + line.x_for_index(marked.end), bounds.bottom()),
                    ),
                    super::design::colors().accent,
                ));
            }
            if range.is_empty() && self.focus.is_focused(window) {
                window.paint_quad(fill(
                    Bounds::new(
                        point(origin.x + caret, bounds.top()),
                        size(caret_width, bounds.size.height),
                    ),
                    super::design::colors().accent,
                ));
            }
        });
        self.line = Some(line);
        self.bounds = Some(bounds);
    }

    fn feedback_position(&self, index: usize) -> Option<Point<Pixels>> {
        if self.feedback_text != self.text.value {
            return None;
        }
        let mut start = 0;
        let mut y = px(0.);
        for line in &self.feedback_lines {
            if index <= start + line.len() {
                return line
                    .position_for_index(index - start, self.line_height)
                    .map(|at| at + point(px(0.), y));
            }
            start += line.len() + 1;
            y += line.size(self.line_height).height;
        }
        None
    }

    fn paint_feedback(
        &mut self,
        bounds: Bounds<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let style = window.text_style();
        let font_size = style.font_size.to_pixels(window.rem_size());
        self.line_height = font_size * 1.5;
        let selected = self.text.selection();
        let mut boundaries = vec![0, self.text.value.len(), selected.start, selected.end];
        if let Some(marked) = &self.text.marked {
            boundaries.extend([marked.start, marked.end]);
        }
        boundaries.sort_unstable();
        boundaries.dedup();
        let runs: Vec<_> = boundaries
            .windows(2)
            .map(|span| TextRun {
                len: span[1] - span[0],
                font: style.font(),
                color: style.color,
                background_color: selected
                    .contains(&span[0])
                    .then_some(super::design::colors().accent_soft.into()),
                underline: self
                    .text
                    .marked
                    .as_ref()
                    .filter(|range| range.contains(&span[0]))
                    .map(|_| gpui::UnderlineStyle {
                        thickness: px(1.),
                        color: Some(super::design::colors().accent.into()),
                        wavy: false,
                    }),
                strikethrough: None,
            })
            .collect();
        let Ok(lines) = window.text_system().shape_text(
            self.text.value.clone().into(),
            font_size,
            &runs,
            Some(bounds.size.width - px(2.)),
            None,
        ) else {
            return;
        };
        self.feedback_lines = lines.into_vec();
        self.feedback_text.clone_from(&self.text.value);
        let caret = self.feedback_position(self.text.cursor).unwrap_or_default();
        self.scroll = self
            .scroll
            .min(caret.y)
            .max(caret.y + self.line_height - bounds.size.height)
            .max(px(0.));
        let mut origin = bounds.origin - point(px(0.), self.scroll);
        window.with_content_mask(Some(gpui::ContentMask { bounds }), |window| {
            for line in &self.feedback_lines {
                let _ = line.paint_background(
                    origin,
                    self.line_height,
                    gpui::TextAlign::Left,
                    None,
                    window,
                    cx,
                );
                let _ = line.paint(
                    origin,
                    self.line_height,
                    gpui::TextAlign::Left,
                    None,
                    window,
                    cx,
                );
                origin.y += line.size(self.line_height).height;
            }
            if selected.is_empty() && self.focus.is_focused(window) {
                window.paint_quad(fill(
                    Bounds::new(
                        bounds.origin + caret - point(px(0.), self.scroll),
                        size(px(1.), self.line_height),
                    ),
                    super::design::colors().accent,
                ));
            }
        });
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
        self.text.anchor = self.text.snap(self.text.anchor);
        self.text.cursor = self.text.snap(self.text.cursor);
        if self.text.feedback {
            cx.emit(NameEvent::Changed(self.text.value.clone()));
        }
        cx.notify();
    }
    fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.read_only {
            return;
        }
        self.text.replace(range, text, false);
        cx.emit(NameEvent::Changed(self.text.value.clone()));
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
        if self.read_only {
            return;
        }
        self.text.compose(range, text, selection);
        cx.emit(NameEvent::Changed(self.text.value.clone()));
        cx.notify();
    }
    fn bounds_for_range(
        &mut self,
        range: Range<usize>,
        bounds: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        if self.text.feedback {
            let range = self.text.bytes(range);
            let at = self.feedback_position(range.start)?;
            return Some(Bounds::new(
                bounds.origin + at - point(px(0.), self.scroll),
                size(px(1.), self.line_height),
            ));
        }
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
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let entity = cx.entity();
        // A name is edited in place, one cell high in the selection's colors,
        // as the source's inline editor is; the chooser's filter keeps a frame.
        let search = self.text.search;
        let field = div()
            .w_full()
            .min_w_0()
            .flex_shrink_0()
            .h(px(if self.text.feedback && self.text.limit.is_some() {
                48.
            } else if self.text.feedback {
                100.
            } else if search {
                super::design::tokens().size.jump_field
            } else {
                super::design::tokens().size.row - super::design::tokens().space.s8
            }))
            .px(px(super::design::tokens().space.s6))
            .rounded(px(super::design::tokens().radius.key))
            .bg(super::design::colors().field)
            .text_color(super::design::colors().text)
            .border(px(super::design::tokens().size.hairline))
            .border_color(
                if self.text.limit.is_none() || self.focus.is_focused(window) {
                    super::design::colors().accent
                } else {
                    super::design::colors().line
                },
            )
            .id("name-field")
            .on_click(cx.listener(|_, _, _, cx| cx.stop_propagation()))
            .track_focus(&self.focus)
            .cursor_text()
            .on_key_down(cx.listener(Self::key))
            .on_action(cx.listener(|field, _: &super::Copy, window, cx| {
                field.key(&super::edit_key("c"), window, cx);
            }))
            .on_action(cx.listener(|field, _: &super::SelectAll, window, cx| {
                field.key(&super::edit_key("a"), window, cx);
            }))
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
                        let released = entity.downgrade();
                        window.on_mouse_event(move |event: &gpui::MouseUpEvent, phase, _, cx| {
                            if phase == gpui::DispatchPhase::Capture
                                && event.button == MouseButton::Left
                            {
                                let _ = released.update(cx, |field, _| field.dragging = false);
                            }
                        });
                        let field = entity.downgrade();
                        window.on_mouse_event(move |event: &gpui::MouseMoveEvent, phase, _, cx| {
                            if phase == gpui::DispatchPhase::Capture {
                                let _ = field.update(cx, |field, cx| {
                                    if field.dragging {
                                        field.text.cursor = field.hit(event.position);
                                        cx.notify();
                                    }
                                });
                            }
                        });
                    },
                )
                .size_full(),
            );
        div()
            .w_full()
            .min_w_0()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .child(field)
            .when(self.text.rejected, |e| {
                e.child(
                    div()
                        .text_color(super::design::colors().attention_text)
                        .child("Input too long; nothing was changed."),
                )
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bounded_values_reject_oversized_paste_and_composition_without_losing_selection() {
        let mut text = Text {
            value: "original".into(),
            anchor: 0,
            cursor: 8,
            limit: Some(16384),
            ..Text::default()
        };
        assert!(text.replace(None, &"x".repeat(16385), false).is_none());
        assert_eq!(text.value, "original");
        assert_eq!(text.selection(), 0..8);
        assert!(text.rejected);
        text.compose(None, &"x".repeat(16385), Some(1..2));
        assert_eq!(text.value, "original");
        assert_eq!(text.selection(), 0..8);
        assert!(text.marked.is_none());
        text.replace(None, "accepted", false);
        assert_eq!(text.value, "accepted");
        assert!(!text.rejected);
    }

    #[test]
    fn feedback_preserves_paragraphs_and_unicode_beyond_the_name_limit() {
        let mut text = Text {
            feedback: true,
            ..Text::default()
        };
        let message = format!("{}\nSecond paragraph 👩‍💻", "Long feedback. ".repeat(20));
        text.replace(None, &message, false);
        assert_eq!(text.value, message);
        assert_eq!(text.cursor, message.len());
        text.replace(
            None,
            &"x".repeat(agent_interface_roster::feedback::TEXT_LIMIT),
            false,
        );
        assert_eq!(
            text.value.chars().count(),
            agent_interface_roster::feedback::TEXT_LIMIT
        );
    }

    #[test]
    fn insertion_snaps_to_the_end_of_a_joined_grapheme_except_during_composition() {
        let mut text = Text {
            value: "🇸".into(),
            ..Text::default()
        };
        text.replace(None, "🇺", false);
        assert_eq!(text.value, "🇺🇸");
        assert_eq!(text.cursor, text.value.len());
        text.anchor = text.boundary(false);
        text.replace(None, "", false);
        assert!(text.value.is_empty());

        text.value = "🇸".into();
        text.compose(None, "🇺", Some(2..2));
        assert_eq!(
            text.cursor, 4,
            "IME offsets retain their exact insertion position"
        );
        text.replace(None, "🇺", false);
        assert_eq!(text.cursor, 8);
    }

    #[test]
    fn composition_selection_is_relative_to_inserted_text_in_utf16() {
        let mut text = Text {
            value: "prefixZ".into(),
            anchor: 6,
            cursor: 7,
            marked: None,
            ..Text::default()
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
        text.search = true;
        text.anchor = 0;
        text.cursor = text.value.len();
        text.replace(None, &"query".repeat(20), false);
        assert_eq!(
            text.value.chars().count(),
            80,
            "search fields retain the reference filter limit"
        );
        text.anchor = 0;
        text.cursor = text.value.len();
        text.replace(None, " Straße\u{a0}\u{200b} ", false);
        assert_eq!(text.value, " Straße ");
    }
}
