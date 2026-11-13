//! A JSON editor for theme files: monospaced, with line numbers, syntax
//! colours from the theme itself, the line of a mistake marked, and undo.

use std::ops::Range;

use gpui::accesskit::{Role, TextPosition, TextSelection};
use gpui::{
    Bounds, ClipboardItem, Context, ElementInputHandler, EntityInputHandler, EventEmitter,
    FocusHandle, MouseButton, Pixels, Point, ScrollWheelEvent, TextRun, UTF16Selection, Window,
    WrappedLine, canvas, div, fill, point, prelude::*, px, size,
};
use unicode_segmentation::UnicodeSegmentation;

use super::design::{colors, tokens};
use super::name::Text;
use super::themes::jsonc::{self, Kind};

pub(super) enum EditorEvent {
    /// The text changed.
    Changed,
    /// Save was asked for, with Command-S or Control-S.
    Save,
}

/// The most edits undo remembers.
const UNDO_LIMIT: usize = 500;

pub(super) struct ThemeEditor {
    text: Text,
    pub focus: FocusHandle,
    /// The text as last laid out, and its lines.
    lines: Vec<WrappedLine>,
    laid_out: String,
    line_height: Pixels,
    /// The text area, right of the line numbers.
    bounds: Option<Bounds<Pixels>>,
    scroll: Pixels,
    /// Bring the caret into view on the next paint.
    follow: bool,
    dragging: bool,
    undo: Vec<(String, usize)>,
    redo: Vec<(String, usize)>,
    /// Whether the last edit was typing that undo joins to the one before.
    typing: bool,
    /// The one-based line of a mistake, marked until the text changes.
    mistake: Option<usize>,
}

impl EventEmitter<EditorEvent> for ThemeEditor {}

impl ThemeEditor {
    pub fn new(value: &str, cx: &mut Context<Self>) -> Self {
        Self {
            text: Text::multiline(value),
            focus: cx.focus_handle(),
            lines: Vec::new(),
            laid_out: String::new(),
            line_height: px(18.),
            bounds: None,
            scroll: px(0.),
            follow: false,
            dragging: false,
            undo: Vec::new(),
            redo: Vec::new(),
            typing: false,
            mistake: None,
        }
    }

    pub fn value(&self) -> &str {
        &self.text.value
    }

    /// Replace the whole text, as one step undo can take back.
    pub fn set_value(&mut self, value: &str, cx: &mut Context<Self>) {
        self.remember(false);
        self.text.value = value.to_owned();
        self.text.anchor = 0;
        self.text.cursor = 0;
        self.text.marked = None;
        self.scroll = px(0.);
        self.mistake = None;
        cx.emit(EditorEvent::Changed);
        cx.notify();
    }

    /// Mark the line of a mistake and move the caret to it.
    pub fn show_mistake(
        &mut self,
        line: Option<usize>,
        column: Option<usize>,
        cx: &mut Context<Self>,
    ) {
        self.mistake = line;
        if let Some(line) = line {
            let at = jsonc::offset(&self.text.value, line, column.unwrap_or(1));
            self.text.anchor = at;
            self.text.cursor = at;
            self.follow = true;
        }
        cx.notify();
    }

    /// Keep the text before an edit for undo. Typing joins one step.
    fn remember(&mut self, typing: bool) {
        if !(typing && self.typing) {
            self.undo.push((self.text.value.clone(), self.text.cursor));
            if self.undo.len() > UNDO_LIMIT {
                self.undo.remove(0);
            }
        }
        self.typing = typing;
        self.redo.clear();
    }

    fn restore(&mut self, redo: bool) {
        let (from, to) = if redo {
            (&mut self.redo, &mut self.undo)
        } else {
            (&mut self.undo, &mut self.redo)
        };
        if let Some((value, cursor)) = from.pop() {
            to.push((
                std::mem::replace(&mut self.text.value, value),
                self.text.cursor,
            ));
            self.text.cursor = cursor.min(self.text.value.len());
            while !self.text.value.is_char_boundary(self.text.cursor) {
                self.text.cursor -= 1;
            }
            self.text.anchor = self.text.cursor;
            self.text.marked = None;
            self.typing = false;
        }
    }

    /// The byte range of the logical line holding `at`, without its break.
    fn line_at(&self, at: usize) -> Range<usize> {
        let value = &self.text.value;
        let start = value[..at].rfind('\n').map_or(0, |at| at + 1);
        let end = value[at..].find('\n').map_or(value.len(), |end| at + end);
        start..end
    }

    fn word_boundary(&self, right: bool) -> usize {
        let value = &self.text.value;
        let cursor = self.text.cursor;
        let words = value
            .split_word_bound_indices()
            .filter(|(_, word)| word.chars().any(char::is_alphanumeric));
        if right {
            words
                .map(|(at, word)| at + word.len())
                .find(|end| *end > cursor)
                .unwrap_or(value.len())
        } else {
            words
                .rev()
                .map(|(at, _)| at)
                .find(|start| *start < cursor)
                .unwrap_or(0)
        }
    }

    /// The caret's place in the laid-out text, from the text area's corner.
    fn position(&self, index: usize) -> Option<Point<Pixels>> {
        if self.laid_out != self.text.value {
            return None;
        }
        let mut start = 0;
        let mut y = px(0.);
        for line in &self.lines {
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

    fn hit(&self, position: Point<Pixels>) -> usize {
        let Some(bounds) = self.bounds.filter(|_| self.laid_out == self.text.value) else {
            return self.text.cursor;
        };
        let mut at = position - bounds.origin + point(px(0.), self.scroll);
        at.x = at.x.max(px(0.));
        if at.y < px(0.) {
            return 0;
        }
        let mut start = 0;
        for line in &self.lines {
            let height = line.size(self.line_height).height;
            if at.y < height {
                let byte = line
                    .closest_index_for_position(at, self.line_height)
                    .unwrap_or_else(|byte| byte);
                return self.text.snap(start + byte);
            }
            start += line.len() + 1;
            at.y -= height;
        }
        self.text.value.len()
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
        let word = if cfg!(target_os = "macos") {
            mods.alt
        } else {
            mods.control
        };
        let plain = !mods.platform && !mods.control && !mods.alt;
        let before = self.text.value.clone();
        let range = self.text.selection();
        let mut moved = true;
        match key.key.as_str() {
            "s" if command => {
                cx.emit(EditorEvent::Save);
                moved = false;
            }
            "z" if command => {
                self.restore(mods.shift);
            }
            "y" if command && cfg!(not(target_os = "macos")) => self.restore(true),
            "a" if command => {
                self.text.anchor = 0;
                self.text.cursor = self.text.value.len();
            }
            "c" | "x" if command => {
                if !range.is_empty() {
                    cx.write_to_clipboard(ClipboardItem::new_string(self.text.value[range].into()));
                    if key.key == "x" {
                        self.remember(false);
                        self.text.replace(None, "", false);
                    }
                }
            }
            "v" if command => {
                if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                    self.remember(false);
                    self.text.replace(None, &text, false);
                }
            }
            "enter" if plain => {
                // Keep the line's indent, one step deeper after an opening bracket.
                let line = self.line_at(self.text.cursor);
                let indent: String = self.text.value[line.start..]
                    .chars()
                    .take_while(|ch| *ch == ' ' || *ch == '\t')
                    .collect();
                let opens = self.text.value[line.start..range.start]
                    .trim_end()
                    .ends_with(['{', '[']);
                let deeper = if opens { "  " } else { "" };
                self.remember(false);
                self.text
                    .replace(None, &format!("\n{indent}{deeper}"), false);
            }
            "tab" if !mods.shift && plain => {
                self.remember(false);
                self.text.replace(None, "  ", false);
            }
            "left" | "right" if !mods.control || cfg!(not(target_os = "macos")) => {
                let right = key.key == "right";
                self.text.cursor = if command && cfg!(target_os = "macos") {
                    let line = self.line_at(self.text.cursor);
                    if right { line.end } else { line.start }
                } else if word {
                    self.word_boundary(right)
                } else if !mods.shift && !range.is_empty() {
                    if right { range.end } else { range.start }
                } else {
                    self.text.boundary(right)
                };
            }
            "home" | "end" => {
                let line = self.line_at(self.text.cursor);
                self.text.cursor = match (key.key.as_str(), command) {
                    ("home", true) => 0,
                    ("end", true) => self.text.value.len(),
                    ("home", false) => line.start,
                    _ => line.end,
                };
            }
            "up" | "down" if command && cfg!(target_os = "macos") => {
                self.text.cursor = if key.key == "up" {
                    0
                } else {
                    self.text.value.len()
                };
            }
            "up" | "down" | "pageup" | "pagedown" if !mods.alt && !mods.control => {
                if let (Some(at), Some(bounds)) = (self.position(self.text.cursor), self.bounds) {
                    let step = match key.key.as_str() {
                        "up" => -self.line_height,
                        "down" => self.line_height,
                        "pageup" => -(bounds.size.height - self.line_height),
                        _ => bounds.size.height - self.line_height,
                    };
                    let target = bounds.origin
                        + at
                        + point(px(1.), step + self.line_height / 2.0 - self.scroll);
                    self.text.cursor = self.hit(target);
                }
            }
            "backspace" | "delete" if !mods.platform && !mods.control => {
                if range.is_empty() {
                    let right = key.key == "delete";
                    self.text.anchor = if mods.alt {
                        self.word_boundary(right)
                    } else {
                        self.text.boundary(right)
                    };
                }
                if self.text.anchor != self.text.cursor {
                    self.remember(false);
                    self.text.replace(None, "", false);
                }
            }
            _ => return,
        }
        let shifting = matches!(
            key.key.as_str(),
            "left" | "right" | "up" | "down" | "home" | "end" | "pageup" | "pagedown"
        );
        if shifting && !mods.shift {
            self.text.anchor = self.text.cursor;
        }
        if self.text.value != before {
            self.mistake = None;
            cx.emit(EditorEvent::Changed);
        }
        self.follow = moved;
        window.prevent_default();
        cx.stop_propagation();
        cx.notify();
    }

    fn paint(&mut self, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let (t, c) = (tokens(), colors());
        let font = gpui::Font {
            family: t.font.mono.clone().into(),
            ..window.text_style().font()
        };
        let font_size = px(t.text.body.size);
        self.line_height = (font_size * 1.6).round();
        // The gutter fits the largest line number, with room on both sides.
        let count = self.text.value.matches('\n').count() + 1;
        let digit = window
            .text_system()
            .shape_line("0".into(), font_size, &[run(1, &font, c.muted, None)], None)
            .width;
        let gutter = digit * (count.to_string().len().max(2) as f32 + 2.0);
        let area = Bounds::from_corners(
            point(bounds.left() + gutter, bounds.top()),
            bounds.bottom_right(),
        );
        let selected = self.text.selection();
        let colour = |kind: Option<Kind>| match kind {
            Some(Kind::Key) => c.info,
            Some(Kind::String) => c.green,
            Some(Kind::Number) => c.code,
            Some(Kind::Literal) => c.violet,
            Some(Kind::Punct) => c.muted,
            Some(Kind::Comment) => c.muted,
            Some(Kind::Invalid) => c.attention_text,
            None => c.text,
        };
        // Runs split at token, selection and composition edges.
        let tokens_found = jsonc::lex(&self.text.value);
        let mut edges = vec![0, self.text.value.len(), selected.start, selected.end];
        if let Some(marked) = &self.text.marked {
            edges.extend([marked.start, marked.end]);
        }
        for token in &tokens_found {
            edges.extend([token.range.start, token.range.end]);
        }
        edges.sort_unstable();
        edges.dedup();
        let mut token = tokens_found.iter().peekable();
        let runs: Vec<TextRun> = edges
            .windows(2)
            .map(|span| {
                while token.peek().is_some_and(|token| token.range.end <= span[0]) {
                    token.next();
                }
                let kind = token
                    .peek()
                    .filter(|token| token.range.contains(&span[0]))
                    .map(|token| token.kind);
                let mut run = run(
                    span[1] - span[0],
                    &font,
                    colour(kind),
                    selected.contains(&span[0]).then_some(c.accent_soft),
                );
                if let Some(marked) = &self.text.marked
                    && marked.contains(&span[0])
                {
                    run.underline = Some(gpui::UnderlineStyle {
                        thickness: px(1.),
                        color: Some(c.accent.into()),
                        wavy: false,
                    });
                }
                run
            })
            .collect();
        let Ok(lines) = window.text_system().shape_text(
            self.text.value.clone().into(),
            font_size,
            &runs,
            Some((area.size.width - digit).max(digit)),
            None,
        ) else {
            return;
        };
        self.lines = lines.into_vec();
        self.laid_out.clone_from(&self.text.value);
        self.bounds = Some(area);
        let total: Pixels = self
            .lines
            .iter()
            .map(|line| line.size(self.line_height).height)
            .sum();
        let caret = self.position(self.text.cursor).unwrap_or_default();
        if self.follow {
            self.scroll = self
                .scroll
                .min(caret.y)
                .max(caret.y + self.line_height - area.size.height);
            self.follow = false;
        }
        self.scroll = self
            .scroll
            .min(total - area.size.height + self.line_height)
            .max(px(0.));
        let focused = self.focus.is_focused(window);
        window.with_content_mask(Some(gpui::ContentMask { bounds }), |window| {
            let mut y = bounds.top() - self.scroll;
            for (number, line) in self.lines.iter().enumerate() {
                let height = line.size(self.line_height).height;
                if y + height >= bounds.top() && y <= bounds.bottom() {
                    if self.mistake == Some(number + 1) {
                        window.paint_quad(fill(
                            Bounds::new(point(bounds.left(), y), size(bounds.size.width, height)),
                            c.attention_soft,
                        ));
                    }
                    let label = (number + 1).to_string();
                    let shaped = window.text_system().shape_line(
                        label.clone().into(),
                        font_size,
                        &[run(
                            label.len(),
                            &font,
                            if self.mistake == Some(number + 1) {
                                c.attention_text
                            } else {
                                c.muted
                            },
                            None,
                        )],
                        None,
                    );
                    let x = area.left() - digit - shaped.width;
                    let _ = shaped.paint(
                        point(x, y),
                        self.line_height,
                        gpui::TextAlign::Left,
                        None,
                        window,
                        cx,
                    );
                    let origin = point(area.left(), y);
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
                }
                y += height;
            }
            if selected.is_empty() && focused {
                window.paint_quad(fill(
                    Bounds::new(
                        area.origin + caret - point(px(0.), self.scroll),
                        size(px(t.size.caret.max(1.)), self.line_height),
                    ),
                    c.accent,
                ));
            }
        });
    }
}

fn run(
    len: usize,
    font: &gpui::Font,
    color: gpui::Rgba,
    background: Option<gpui::Rgba>,
) -> TextRun {
    TextRun {
        len,
        font: font.clone(),
        color: color.into(),
        background_color: background.map(Into::into),
        underline: None,
        strikethrough: None,
    }
}

impl EntityInputHandler for ThemeEditor {
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
        cx.notify();
    }

    fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let typing = self.text.marked.is_none()
            && text.chars().count() == 1
            && !text.chars().all(char::is_whitespace);
        self.remember(typing);
        self.text.replace(range, text, false);
        self.mistake = None;
        self.follow = true;
        cx.emit(EditorEvent::Changed);
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
        if self.text.marked.is_none() {
            self.remember(false);
        }
        self.text.compose(range, text, selection);
        self.follow = true;
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range: Range<usize>,
        _: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let area = self.bounds?;
        let range = self.text.bytes(range);
        let at = self.position(range.start)?;
        Some(Bounds::new(
            area.origin + at - point(px(0.), self.scroll),
            size(px(1.), self.line_height),
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

impl Render for ThemeEditor {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let entity = cx.entity();
        div()
            .id("theme-editor")
            .size_full()
            .min_w_0()
            .role(Role::MultilineTextInput)
            .aria_label("Theme JSON")
            .a11y_synthetic_children({
                let (run, anchor, caret) = self.text.accessible_run();
                move |builder| {
                    let id = builder.synthetic_node_id(0);
                    builder.push_child(id, run);
                    builder.parent_node().set_text_selection(TextSelection {
                        anchor: TextPosition {
                            node: id,
                            character_index: anchor,
                        },
                        focus: TextPosition {
                            node: id,
                            character_index: caret,
                        },
                    });
                }
            })
            .track_focus(&self.focus)
            .cursor_text()
            .on_key_down(cx.listener(Self::key))
            .on_action(cx.listener(|editor, _: &super::Copy, window, cx| {
                editor.key(&super::edit_key("c"), window, cx);
            }))
            .on_action(cx.listener(|editor, _: &super::SelectAll, window, cx| {
                editor.key(&super::edit_key("a"), window, cx);
            }))
            .on_scroll_wheel(cx.listener(|editor, event: &ScrollWheelEvent, _, cx| {
                let delta = event.delta.pixel_delta(editor.line_height);
                editor.scroll = (editor.scroll - delta.y).max(px(0.));
                cx.stop_propagation();
                cx.notify();
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|editor, event: &gpui::MouseDownEvent, window, cx| {
                    window.focus(&editor.focus, cx);
                    editor.text.cursor = editor.hit(event.position);
                    if event.click_count == 2 {
                        let value = &editor.text.value;
                        if let Some((at, word)) = value
                            .split_word_bound_indices()
                            .find(|(at, word)| (*at..at + word.len()).contains(&editor.text.cursor))
                        {
                            editor.text.anchor = at;
                            editor.text.cursor = at + word.len();
                        }
                    } else if !event.modifiers.shift {
                        editor.text.anchor = editor.text.cursor;
                    }
                    editor.dragging = event.click_count < 2;
                    cx.stop_propagation();
                    cx.notify();
                }),
            )
            .child(
                canvas(
                    |_, _, _| (),
                    move |bounds, _, window, cx| {
                        let focus = entity.read(cx).focus.clone();
                        entity.update(cx, |editor, cx| editor.paint(bounds, window, cx));
                        let area = entity.read(cx).bounds.unwrap_or(bounds);
                        window.handle_input(
                            &focus,
                            super::zoom::input(ElementInputHandler::new(area, entity.clone())),
                            cx,
                        );
                        let released = entity.downgrade();
                        window.on_mouse_event(move |event: &gpui::MouseUpEvent, phase, _, cx| {
                            if phase == gpui::DispatchPhase::Capture
                                && event.button == MouseButton::Left
                            {
                                let _ = released.update(cx, |editor, _| editor.dragging = false);
                            }
                        });
                        let dragged = entity.downgrade();
                        window.on_mouse_event(move |event: &gpui::MouseMoveEvent, phase, _, cx| {
                            if phase == gpui::DispatchPhase::Capture {
                                let _ = dragged.update(cx, |editor, cx| {
                                    if editor.dragging {
                                        editor.text.cursor = editor.hit(event.position);
                                        editor.follow = true;
                                        cx.notify();
                                    }
                                });
                            }
                        });
                    },
                )
                .size_full(),
            )
    }
}
