//! Shared native selection for rendered documents and their source text.

use std::{cell::RefCell, ops::Range};

use gpui::{
    Bounds, ClipboardItem, Context, HighlightStyle, Pixels, Point, StyledText, TextLayout, canvas,
    div, prelude::*,
};
use unicode_segmentation::UnicodeSegmentation;

use super::{ViewPane, markdown::Mark};
use crate::desktop::design::colors;

/// A view whose rendered text can be selected and copied.
pub(super) trait Selectable: 'static {
    fn selection(&self) -> &RefCell<Selection>;
}

impl Selectable for ViewPane {
    fn selection(&self) -> &RefCell<Selection> {
        &self.selection
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Position {
    part: usize,
    byte: usize,
}

struct Part {
    text: String,
    /// Links shown as short labels: each label's range and its address,
    /// which a copy gives back in place of the label.
    links: Vec<(Range<usize>, String)>,
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
    invalidated: bool,
    /// A link pressed and not yet released; it opens on release unless the
    /// press became a selection.
    pressed_link: Option<String>,
}

impl Selection {
    pub fn begin(&mut self) {
        self.next = 0;
        self.invalidated = false;
    }

    pub fn finish(&mut self) -> bool {
        if self.next != self.parts.len() {
            self.clear();
            self.parts.truncate(self.next);
        }
        self.invalidated
    }

    fn clear(&mut self) {
        self.invalidated |= self.anchor.is_some() || self.cursor.is_some();
        self.anchor = None;
        self.cursor = None;
        self.dragging = false;
    }

    fn register(&mut self, text: &str, links: &[(Range<usize>, String)]) -> usize {
        let index = self.next;
        self.next += 1;
        if self.parts.get(index).is_none_or(|part| part.text != text) {
            self.clear();
        }
        let part = Part {
            text: text.into(),
            links: links.to_vec(),
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
                (!range.is_empty()).then(|| copied(part, range))
            })
            .collect::<Vec<_>>()
            .join("\n\n")
    }

    /// The link at `position`, if a label is there.
    fn link_at(&self, position: Position) -> Option<String> {
        self.parts[position.part]
            .links
            .iter()
            .find(|(range, _)| range.contains(&position.byte))
            .map(|(_, url)| url.clone())
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
        if clicks >= 3 {
            // A triple click selects the whole paragraph, item or cell.
            self.anchor = Some(Position {
                byte: 0,
                ..position
            });
            self.cursor = Some(Position {
                byte: self.parts[position.part].text.len(),
                ..position
            });
        } else if clicks == 2 {
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

    /// Press at `point` beside the text, such as between paragraphs or on a
    /// list marker: as on the nearest text.
    fn press_near(&mut self, point: Point<Pixels>, shift: bool, clicks: usize) -> bool {
        let Some(index) = self.nearest(point) else {
            return false;
        };
        self.press(self.hit(index, point), shift, clicks);
        true
    }

    fn motion(&mut self, point: Point<Pixels>) -> bool {
        if !self.dragging {
            return false;
        }
        let Some(index) = self.nearest(point) else {
            return false;
        };
        let cursor = self.hit(index, point);
        if self.cursor == Some(cursor) {
            return false;
        }
        self.cursor = Some(cursor);
        true
    }

    /// The nearest rendered block, so dragging can cross paragraphs, headings
    /// and code blocks without losing the original anchor.
    fn nearest(&self, point: Point<Pixels>) -> Option<usize> {
        self.parts
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
            .map(|(index, _)| index)
    }

    pub fn key<V: 'static>(&mut self, key: &gpui::Keystroke, cx: &mut Context<V>) -> bool {
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
                self.select_all();
                cx.notify();
            }
            "c" => self.copy(cx),
            _ => return false,
        }
        true
    }

    /// Select every block shown.
    pub fn select_all(&mut self) {
        let Some(last) = self.parts.len().checked_sub(1) else {
            return;
        };
        self.anchor = Some(Position { part: 0, byte: 0 });
        self.cursor = Some(Position {
            part: last,
            byte: self.parts[last].text.len(),
        });
    }

    /// Put the selected text on the clipboard, if any is selected.
    pub fn copy(&self, cx: &mut gpui::App) {
        let text = self.selected_text();
        if !text.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }
}

/// The text of `range` in `part`, with each link label it touches given
/// back as the link's address.
fn copied(part: &Part, range: Range<usize>) -> String {
    let mut text = String::new();
    let mut at = range.start;
    for (label, url) in &part.links {
        if label.end <= range.start || label.start >= range.end {
            continue;
        }
        if label.start > at {
            text.push_str(&part.text[at..label.start]);
        }
        text.push_str(url);
        at = label.end.min(range.end).max(at);
    }
    if at < range.end {
        text.push_str(&part.text[at..range.end]);
    }
    text
}

/// One block with the existing typography and its inline styles. The caller
/// supplies its outer styles; selection adds no field chrome or editing.
pub(super) fn text<V: Selectable>(
    view: &V,
    text: &str,
    marks: &[(Range<usize>, Mark)],
    cx: &mut Context<V>,
) -> gpui::Stateful<gpui::Div> {
    linked(view, text, marks, &[], cx)
}

/// [`text`] with links: each label in `links` is drawn in the accent and
/// opens its address when clicked.
pub(super) fn linked<V: Selectable>(
    view: &V,
    text: &str,
    marks: &[(Range<usize>, Mark)],
    links: &[(Range<usize>, String)],
    cx: &mut Context<V>,
) -> gpui::Stateful<gpui::Div> {
    let index = view.selection().borrow_mut().register(text, links);
    let selected = view.selection().borrow().range(index);
    let c = colors();
    let mut boundaries = vec![0, text.len(), selected.start, selected.end];
    for range in marks
        .iter()
        .map(|(range, _)| range)
        .chain(links.iter().map(|(range, _)| range))
    {
        boundaries.extend([range.start, range.end]);
    }
    boundaries.sort_unstable();
    boundaries.dedup();
    // Marks may nest, as bold inside a link; each span takes every mark over it.
    let highlights = boundaries.windows(2).map(|span| {
        let range = span[0]..span[1];
        let mut style = HighlightStyle::default();
        for (_, mark) in marks.iter().filter(|(mark, _)| mark.contains(&range.start)) {
            mark.apply(&mut style);
        }
        if links.iter().any(|(link, _)| link.contains(&range.start)) {
            style.color = Some(c.accent_text.into());
        }
        if selected.contains(&range.start) {
            // Selected text keeps its own colour over a translucent accent.
            style.background_color = Some(
                gpui::Rgba {
                    a: 0.35,
                    ..c.accent
                }
                .into(),
            );
        }
        (range, style)
    });
    let styled = StyledText::new(text.to_owned()).with_highlights(highlights);
    view.selection().borrow_mut().parts[index].layout = styled.layout().clone();
    let entity = cx.entity().downgrade();
    div()
        .id(("selectable-text", index))
        .relative()
        .cursor_text()
        .on_mouse_down(
            gpui::MouseButton::Left,
            cx.listener(move |view: &mut V, event: &gpui::MouseDownEvent, _, cx| {
                let mut selection = view.selection().borrow_mut();
                let at = selection.hit(index, event.position);
                selection.pressed_link = (event.click_count == 1 && !event.modifiers.shift)
                    .then(|| selection.link_at(at))
                    .flatten();
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
                    let _ = entity.update(cx, |view, _| {
                        if let Some(part) = view.selection().borrow_mut().parts.get_mut(index) {
                            part.bounds = Some(bounds);
                        }
                    });
                },
            )
            .absolute()
            .size_full(),
        )
}

/// Start a selection from a press anywhere in `element` that is not on text,
/// such as the space between paragraphs or a list marker.
pub(super) fn area<V: Selectable>(element: gpui::Div, cx: &mut Context<V>) -> gpui::Div {
    element.on_mouse_down(
        gpui::MouseButton::Left,
        cx.listener(|view: &mut V, event: &gpui::MouseDownEvent, _, cx| {
            let pressed = view.selection().borrow_mut().press_near(
                event.position,
                event.modifiers.shift,
                event.click_count,
            );
            if pressed {
                cx.notify();
            }
        }),
    )
}

/// Keep drag selection and release working beyond the initial text block.
pub(super) fn events<V: Selectable>(cx: &Context<V>) -> impl IntoElement {
    let entity = cx.entity().downgrade();
    canvas(
        |_, _, _| (),
        move |_, (), window, _| {
            let moving = entity.clone();
            window.on_mouse_event(move |event: &gpui::MouseMoveEvent, phase, _, cx| {
                if phase == gpui::DispatchPhase::Capture {
                    let _ = moving.update(cx, |view, cx| {
                        let mut selection = view.selection().borrow_mut();
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
                    let _ = released.update(cx, |view, cx| {
                        let mut selection = view.selection().borrow_mut();
                        selection.dragging = false;
                        // A press on a link that selected nothing opens it.
                        if let Some(url) = selection.pressed_link.take()
                            && selection.selected_text().is_empty()
                        {
                            cx.open_url(&url);
                        }
                    });
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
        selection.register("First café", &[]);
        selection.register("Second 👩‍💻 block", &[]);
        selection.anchor = Some(Position { part: 0, byte: 6 });
        selection.cursor = Some(Position {
            part: 1,
            byte: "Second 👩‍💻".len(),
        });
        assert_eq!(selection.selected_text(), "café\n\nSecond 👩‍💻");
        std::mem::swap(&mut selection.anchor, &mut selection.cursor);
        assert_eq!(selection.selected_text(), "café\n\nSecond 👩‍💻");
        selection.begin();
        selection.register("Another document", &[]);
        selection.finish();
        assert_eq!(selection.selected_text(), "");
    }

    #[test]
    fn a_copied_link_label_gives_back_its_address() {
        let mut selection = Selection::default();
        let text = "See x.com ↗ now";
        let label = 4.."See x.com ↗".len();
        selection.register(text, &[(label, "https://x.com/a".into())]);
        selection.anchor = Some(Position { part: 0, byte: 0 });
        selection.cursor = Some(Position {
            part: 0,
            byte: text.len(),
        });
        assert_eq!(selection.selected_text(), "See https://x.com/a now");
        // Part of a label still copies the whole address.
        selection.anchor = Some(Position { part: 0, byte: 6 });
        assert_eq!(selection.selected_text(), "https://x.com/a now");
        assert_eq!(
            selection.link_at(Position { part: 0, byte: 5 }).as_deref(),
            Some("https://x.com/a")
        );
    }

    #[test]
    fn double_click_selects_a_word_shift_click_extends_and_triple_click_takes_the_block() {
        let mut selection = Selection::default();
        selection.register("Read café here", &[]);
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
        selection.press(Position { part: 0, byte: 6 }, false, 3);
        assert_eq!(selection.selected_text(), "Read café here");
    }

    #[test]
    fn later_changes_rebuild_earlier_highlights_before_caching() {
        let mut selection = Selection::default();
        for later in [Some("Changed later"), None] {
            selection.begin();
            selection.register("Selected first", &[]);
            selection.register("Original later", &[]);
            selection.finish();
            selection.press(Position { part: 0, byte: 1 }, false, 2);

            selection.begin();
            selection.register("Selected first", &[]);
            assert!(!selection.range(0).is_empty());
            if let Some(later) = later {
                selection.register(later, &[]);
            }
            assert!(selection.finish());

            selection.begin();
            selection.register("Selected first", &[]);
            assert!(selection.range(0).is_empty());
            if let Some(later) = later {
                selection.register(later, &[]);
            }
            assert!(!selection.finish());
            assert!(selection.selected_text().is_empty());
        }
    }
}
