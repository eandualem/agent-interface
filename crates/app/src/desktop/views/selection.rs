//! Shared native selection for rendered documents and their source text.

use std::ops::Range;

use gpui::{
    Bounds, ClipboardItem, Context, HighlightStyle, Pixels, Point, StyledText, TextLayout, canvas,
    div, prelude::*,
};
use unicode_segmentation::UnicodeSegmentation;

use super::ViewPane;
use crate::desktop::design::colors;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Position {
    part: usize,
    byte: usize,
}

struct Part {
    text: String,
    layout: TextLayout,
    bounds: Option<Bounds<Pixels>>,
}

#[derive(Default)]
pub(super) struct Selection {
    parts: Vec<Part>,
    next: usize,
    anchor: Option<Position>,
    cursor: Option<Position>,
    dragging: bool,
}

impl Selection {
    pub fn begin(&mut self) {
        self.next = 0;
    }

    pub fn finish(&mut self) {
        if self.next != self.parts.len() {
            self.clear();
            self.parts.truncate(self.next);
        }
    }

    fn clear(&mut self) {
        self.anchor = None;
        self.cursor = None;
        self.dragging = false;
    }

    fn register(&mut self, text: &str) -> usize {
        let index = self.next;
        self.next += 1;
        if self.parts.get(index).is_none_or(|part| part.text != text) {
            self.clear();
        }
        let part = Part {
            text: text.into(),
            layout: TextLayout::default(),
            bounds: None,
        };
        if index < self.parts.len() {
            self.parts[index] = part;
        } else {
            self.parts.push(part);
        }
        index
    }

    fn range(&self, index: usize) -> Range<usize> {
        let Some((anchor, cursor)) = self.anchor.zip(self.cursor) else {
            return 0..0;
        };
        let (start, end) = (anchor.min(cursor), anchor.max(cursor));
        if index < start.part || index > end.part {
            return 0..0;
        }
        let length = self.parts[index].text.len();
        let start = if index == start.part { start.byte } else { 0 };
        let end = if index == end.part { end.byte } else { length };
        start.min(length)..end.min(length)
    }

    fn selected_text(&self) -> String {
        self.parts
            .iter()
            .enumerate()
            .filter_map(|(index, part)| {
                let range = self.range(index);
                (!range.is_empty()).then(|| &part.text[range])
            })
            .collect::<Vec<_>>()
            .join("\n\n")
    }

    /// TextLayout reports UTF-8 byte offsets. Snap to a whole grapheme before
    /// slicing, including combining marks and multi-codepoint emoji.
    fn hit(&self, index: usize, point: Point<Pixels>) -> Position {
        let part = &self.parts[index];
        let byte = part
            .layout
            .index_for_position(point)
            .unwrap_or_else(|byte| byte);
        let byte = part
            .text
            .grapheme_indices(true)
            .map(|(i, _)| i)
            .chain(std::iter::once(part.text.len()))
            .take_while(|i| *i <= byte)
            .last()
            .unwrap_or(0);
        Position { part: index, byte }
    }

    fn press(&mut self, position: Position, shift: bool, clicks: usize) {
        self.cursor = Some(position);
        if !shift || self.anchor.is_none() {
            self.anchor = Some(position);
        }
        if clicks == 2 {
            let text = &self.parts[position.part].text;
            if let Some((start, word)) = text
                .split_word_bound_indices()
                .find(|(start, word)| (*start..*start + word.len()).contains(&position.byte))
            {
                self.anchor = Some(Position {
                    byte: start,
                    ..position
                });
                self.cursor = Some(Position {
                    byte: start + word.len(),
                    ..position
                });
            }
        }
        self.dragging = true;
    }

    fn motion(&mut self, point: Point<Pixels>) -> bool {
        if !self.dragging {
            return false;
        }
        // Choose the nearest rendered block, so dragging can cross paragraphs,
        // headings and code blocks without losing the original anchor.
        let index = self
            .parts
            .iter()
            .enumerate()
            .filter_map(|(index, part)| {
                let bounds = part.bounds?;
                let distance = if point.y < bounds.top() {
                    bounds.top() - point.y
                } else if point.y > bounds.bottom() {
                    point.y - bounds.bottom()
                } else {
                    gpui::px(0.)
                };
                Some((index, distance))
            })
            .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap())
            .map(|(index, _)| index);
        let Some(index) = index else { return false };
        let cursor = self.hit(index, point);
        if self.cursor == Some(cursor) {
            return false;
        }
        self.cursor = Some(cursor);
        true
    }

    pub fn key(&mut self, key: &gpui::Keystroke, cx: &mut Context<ViewPane>) -> bool {
        let mods = key.modifiers;
        let command = if cfg!(target_os = "macos") {
            mods.platform && !mods.control
        } else {
            mods.control && !mods.platform
        };
        if !command || mods.alt || mods.function || mods.shift || self.parts.is_empty() {
            return false;
        }
        match key.key.as_str() {
            "a" => {
                self.anchor = Some(Position { part: 0, byte: 0 });
                let last = self.parts.len() - 1;
                self.cursor = Some(Position {
                    part: last,
                    byte: self.parts[last].text.len(),
                });
                cx.notify();
            }
            "c" => {
                let text = self.selected_text();
                if !text.is_empty() {
                    cx.write_to_clipboard(ClipboardItem::new_string(text));
                }
            }
            _ => return false,
        }
        true
    }
}

/// One block with the existing typography and inline-code styling. The caller
/// supplies its outer styles; selection adds no field chrome or editing.
pub(super) fn text(
    pane: &ViewPane,
    text: &str,
    code: &[Range<usize>],
    cx: &mut Context<ViewPane>,
) -> gpui::Stateful<gpui::Div> {
    let index = pane.selection.borrow_mut().register(text);
    let selected = pane.selection.borrow().range(index);
    let c = colors();
    let mut boundaries = vec![0, text.len(), selected.start, selected.end];
    for range in code {
        boundaries.extend([range.start, range.end]);
    }
    boundaries.sort_unstable();
    boundaries.dedup();
    let highlights = boundaries.windows(2).map(|span| {
        let range = span[0]..span[1];
        let style = if selected.contains(&range.start) {
            HighlightStyle {
                color: Some(c.on_accent.into()),
                background_color: Some(c.accent.into()),
                ..Default::default()
            }
        } else if code.iter().any(|code| code.contains(&range.start)) {
            HighlightStyle {
                color: Some(c.code.into()),
                background_color: Some(c.selected.into()),
                ..Default::default()
            }
        } else {
            HighlightStyle::default()
        };
        (range, style)
    });
    let styled = StyledText::new(text.to_owned()).with_highlights(highlights);
    pane.selection.borrow_mut().parts[index].layout = styled.layout().clone();
    let entity = cx.entity().downgrade();
    div()
        .id(("selectable-text", index))
        .relative()
        .cursor_text()
        .on_mouse_down(
            gpui::MouseButton::Left,
            cx.listener(move |pane, event: &gpui::MouseDownEvent, _, cx| {
                let mut selection = pane.selection.borrow_mut();
                let at = selection.hit(index, event.position);
                selection.press(at, event.modifiers.shift, event.click_count);
                cx.stop_propagation();
                cx.notify();
            }),
        )
        .child(styled)
        .child(
            canvas(
                |_, _, _| (),
                move |bounds, (), _, cx| {
                    let _ = entity.update(cx, |pane, _| {
                        if let Some(part) = pane.selection.borrow_mut().parts.get_mut(index) {
                            part.bounds = Some(bounds);
                        }
                    });
                },
            )
            .absolute()
            .size_full(),
        )
}

/// Keep drag selection and release working beyond the initial text block.
pub(super) fn events(cx: &Context<ViewPane>) -> impl IntoElement {
    let entity = cx.entity().downgrade();
    canvas(
        |_, _, _| (),
        move |_, (), window, _| {
            let moving = entity.clone();
            window.on_mouse_event(move |event: &gpui::MouseMoveEvent, phase, _, cx| {
                if phase == gpui::DispatchPhase::Capture {
                    let _ = moving.update(cx, |pane, cx| {
                        let mut selection = pane.selection.borrow_mut();
                        if !event.dragging() {
                            selection.dragging = false;
                        } else if selection.motion(event.position) {
                            cx.notify();
                        }
                    });
                }
            });
            let released = entity.clone();
            window.on_mouse_event(move |event: &gpui::MouseUpEvent, phase, _, cx| {
                if phase == gpui::DispatchPhase::Capture && event.button == gpui::MouseButton::Left
                {
                    let _ =
                        released.update(cx, |pane, _| pane.selection.borrow_mut().dragging = false);
                }
            });
        },
    )
    .absolute()
    .size_full()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_copies_across_blocks_in_both_directions_and_resets_on_new_text() {
        let mut selection = Selection::default();
        selection.register("First café");
        selection.register("Second 👩‍💻 block");
        selection.anchor = Some(Position { part: 0, byte: 6 });
        selection.cursor = Some(Position {
            part: 1,
            byte: "Second 👩‍💻".len(),
        });
        assert_eq!(selection.selected_text(), "café\n\nSecond 👩‍💻");
        std::mem::swap(&mut selection.anchor, &mut selection.cursor);
        assert_eq!(selection.selected_text(), "café\n\nSecond 👩‍💻");
        selection.begin();
        selection.register("Another document");
        selection.finish();
        assert_eq!(selection.selected_text(), "");
    }

    #[test]
    fn double_click_selects_a_word_and_shift_click_extends_it() {
        let mut selection = Selection::default();
        selection.register("Read café here");
        selection.press(Position { part: 0, byte: 6 }, false, 2);
        assert_eq!(selection.selected_text(), "café");
        selection.press(
            Position {
                part: 0,
                byte: "Read café here".len(),
            },
            true,
            1,
        );
        assert_eq!(selection.selected_text(), "café here");
    }
}
