//! A small reader for skill documents: headings, paragraphs, lists, code
//! blocks and inline code, drawn in the document styles. Web addresses read
//! as their site and open when clicked; it loads no images.

use std::ops::Range;

use gpui::{Context, div, prelude::*, px};

use super::{ViewPane, selection};

use crate::desktop::{
    components as ui,
    design::{TextStyle, colors, tokens},
};

/// How a run of blocks reads: its text style, colour, line height and the
/// space between blocks. A document uses [`render`]; a reader's short
/// authored field, such as a report's goal, passes its own.
pub(super) struct Look<'a> {
    pub style: &'a TextStyle,
    pub color: gpui::Rgba,
    pub line_height: f32,
    pub gap: f32,
}

#[derive(Debug, PartialEq)]
pub(super) enum Block {
    Heading(u8, String),
    Paragraph(String),
    List(Vec<String>),
    Code(String),
}

/// The blocks of `text`, after any front matter between `---` lines.
pub(super) fn parse(text: &str) -> Vec<Block> {
    parse_text(text, true)
}

/// Authored records contain no skill front matter; preserve all their text.
pub(super) fn parse_authored(text: &str) -> Vec<Block> {
    parse_text(text, false)
}

fn parse_text(text: &str, front_matter: bool) -> Vec<Block> {
    let mut lines = text.lines().peekable();
    // Front matter only when a closing line follows; otherwise `---` is a rule.
    if front_matter
        && lines.peek().is_some_and(|line| line.trim() == "---")
        && text.lines().skip(1).any(|line| line.trim() == "---")
    {
        lines.next();
        for line in lines.by_ref() {
            if line.trim() == "---" {
                break;
            }
        }
    }
    let mut blocks = Vec::new();
    let mut paragraph = Vec::new();
    let mut items: Vec<String> = Vec::new();
    let flush = |blocks: &mut Vec<Block>, paragraph: &mut Vec<String>, items: &mut Vec<String>| {
        if !paragraph.is_empty() {
            blocks.push(Block::Paragraph(paragraph.join(" ")));
            paragraph.clear();
        }
        if !items.is_empty() {
            blocks.push(Block::List(std::mem::take(items)));
        }
    };
    while let Some(line) = lines.next() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") {
            flush(&mut blocks, &mut paragraph, &mut items);
            let mut code = Vec::new();
            for line in lines.by_ref() {
                if line.trim().starts_with("```") {
                    break;
                }
                code.push(line);
            }
            blocks.push(Block::Code(code.join("\n")));
        } else if let Some(level) =
            (1..=6).find(|n| trimmed.starts_with(&"#".repeat(*n)) && trimmed[*n..].starts_with(' '))
        {
            flush(&mut blocks, &mut paragraph, &mut items);
            blocks.push(Block::Heading(
                level as u8,
                trimmed[level + 1..].trim().to_owned(),
            ));
        } else if let Some(item) = ["- ", "* ", "+ "]
            .iter()
            .find_map(|mark| trimmed.strip_prefix(mark))
            .or_else(|| {
                let digits = trimmed.find(|ch: char| !ch.is_ascii_digit())?;
                (digits > 0)
                    .then(|| trimmed[digits..].strip_prefix(". "))
                    .flatten()
            })
        {
            if !paragraph.is_empty() {
                blocks.push(Block::Paragraph(paragraph.join(" ")));
                paragraph.clear();
            }
            items.push(item.to_owned());
        } else if trimmed.is_empty() {
            flush(&mut blocks, &mut paragraph, &mut items);
        } else if !items.is_empty() && line.starts_with([' ', '\t']) {
            // A continued list item.
            if let Some(last) = items.last_mut() {
                last.push(' ');
                last.push_str(trimmed);
            }
        } else {
            if !items.is_empty() {
                blocks.push(Block::List(std::mem::take(&mut items)));
            }
            paragraph.push(trimmed.to_owned());
        }
    }
    flush(&mut blocks, &mut paragraph, &mut items);
    blocks
}

/// A link as it reads in a sentence: its site and an arrow.
type Links = Vec<(Range<usize>, String)>;

/// `text` with its `code` spans marked and the backticks removed, and each
/// web address outside code shortened to its site, such as `x.com ↗`.
fn inline(text: &str) -> (String, Vec<Range<usize>>, Links) {
    let mut plain = String::new();
    let mut spans = Vec::new();
    let mut links = Vec::new();
    for (index, part) in text.split('`').enumerate() {
        let start = plain.len();
        if index % 2 == 1 {
            plain.push_str(part);
            if !part.is_empty() {
                spans.push(start..plain.len());
            }
        } else {
            shorten(part, &mut plain, &mut links);
        }
    }
    (plain, spans, links)
}

/// The web address at the start of `text`. It ends at a space or an angle
/// bracket, as in `<https://…>`; closing punctuation belongs to the sentence,
/// and a closing parenthesis too unless the address opened it, as in
/// `https://en.wikipedia.org/wiki/Rust_(programming_language)`.
fn address(text: &str) -> &str {
    let end = text
        .find(|ch: char| ch.is_whitespace() || ch == '<' || ch == '>')
        .unwrap_or(text.len());
    let mut url = &text[..end];
    loop {
        let trimmed = url.trim_end_matches(['.', ',', ';', ':', '!', '?', '\'', '"']);
        let unopened =
            trimmed.ends_with(')') && trimmed.matches('(').count() < trimmed.matches(')').count();
        let next = if unopened {
            &trimmed[..trimmed.len() - 1]
        } else {
            trimmed
        };
        if next.len() == url.len() {
            return url;
        }
        url = next;
    }
}

/// Append `text` to `plain`, each address in it as its site.
fn shorten(mut text: &str, plain: &mut String, links: &mut Links) {
    while let Some(at) = ["https://", "http://"]
        .iter()
        .filter_map(|scheme| text.find(scheme))
        .min()
    {
        plain.push_str(&text[..at]);
        let rest = &text[at..];
        let url = address(rest);
        let site = url
            .split_once("://")
            .map_or(url, |(_, rest)| rest)
            .split(['/', '?', '#'])
            .next()
            .unwrap_or_default();
        let site = site.strip_prefix("www.").unwrap_or(site);
        if site.is_empty() {
            plain.push_str(url);
        } else {
            let start = plain.len();
            plain.push_str(site);
            plain.push_str(" ↗");
            links.push((start..plain.len(), url.to_owned()));
        }
        text = &rest[url.len()..];
    }
    plain.push_str(text);
}

fn styled(text: &str, pane: &ViewPane, cx: &mut Context<ViewPane>) -> impl IntoElement {
    let (plain, spans, links) = inline(text);
    selection::linked(pane, &plain, &spans, &links, cx)
}

/// Draw the blocks as a document.
pub(super) fn render(blocks: &[Block], pane: &ViewPane, cx: &mut Context<ViewPane>) -> gpui::Div {
    let (t, c) = (tokens(), colors());
    let look = Look {
        style: &t.text.doc_body,
        color: c.text_2,
        line_height: 1.6,
        gap: t.space.s12,
    };
    render_in(blocks, &look, pane, cx)
}

/// Draw the blocks with `look`.
pub(super) fn render_in(
    blocks: &[Block],
    look: &Look,
    pane: &ViewPane,
    cx: &mut Context<ViewPane>,
) -> gpui::Div {
    let (t, c) = (tokens(), colors());
    let mut document = ui::text(
        div()
            .flex()
            .flex_col()
            .gap(px(look.gap))
            .text_color(look.color)
            .line_height(gpui::relative(look.line_height)),
        look.style,
    );
    for block in blocks {
        document = document.child(match block {
            Block::Heading(1, text) => ui::text(div().text_color(c.text), &t.text.doc_title)
                .child(styled(text, pane, cx))
                .into_any_element(),
            Block::Heading(_, text) => ui::text(
                div().mt(px(t.space.s4)).text_color(c.text),
                &t.text.doc_heading,
            )
            .child(styled(text, pane, cx))
            .into_any_element(),
            Block::Paragraph(text) => div().child(styled(text, pane, cx)).into_any_element(),
            Block::List(items) => {
                let mut list = div()
                    .flex()
                    .flex_col()
                    .gap(px(t.space.s2))
                    .pl(px(t.space.s8));
                for item in items {
                    list = list.child(
                        div()
                            .flex()
                            .gap(px(t.space.s8))
                            .child(div().flex_shrink_0().text_color(c.muted).child("•"))
                            .child(div().flex_1().min_w_0().child(styled(item, pane, cx))),
                    );
                }
                list.into_any_element()
            }
            Block::Code(code) => ui::text(
                div()
                    .px(px(t.space.s14))
                    .py(px(t.space.s12))
                    .rounded(px(t.radius.slot))
                    .bg(c.ground)
                    .border(px(t.size.hairline))
                    .border_color(c.line)
                    .font_family(t.font.mono.clone())
                    .text_color(c.text_2)
                    .overflow_hidden(),
                &t.text.status,
            )
            .child(selection::text(pane, code, &[], cx))
            .into_any_element(),
        });
    }
    document
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authored_rules_do_not_hide_the_text_between_them() {
        assert_eq!(
            parse_authored("---\nKeep this answer\n---\nAfter"),
            [Block::Paragraph("--- Keep this answer --- After".into())]
        );
    }

    #[test]
    fn documents_split_into_blocks_without_front_matter() {
        let text = "---\nname: check\n---\n# Check\n\nRun it `before` pushing,\nalways.\n\n## When\n- one\n- two\n  more\n1. three\n\n```bash\nrun --now\n```\nafter";
        assert_eq!(
            parse(text),
            [
                Block::Heading(1, "Check".into()),
                Block::Paragraph("Run it `before` pushing, always.".into()),
                Block::Heading(2, "When".into()),
                Block::List(vec!["one".into(), "two more".into(), "three".into()]),
                Block::Code("run --now".into()),
                Block::Paragraph("after".into()),
            ]
        );
        assert_eq!(parse("---\nText"), [Block::Paragraph("--- Text".into())]);
        let (plain, spans, _) = inline("a `b` c");
        assert_eq!(plain, "a b c");
        assert_eq!(spans.first(), Some(&(2..3)));
    }

    #[test]
    fn web_addresses_read_as_their_site() {
        let (plain, spans, links) =
            inline("See https://www.example.com/a/b?c=1. Run `https://x.y` too");
        assert_eq!(plain, "See example.com ↗. Run https://x.y too");
        assert_eq!(spans.first(), Some(&(25..36)));
        assert_eq!(
            address("https://en.wikipedia.org/wiki/Rust_(lang)."),
            "https://en.wikipedia.org/wiki/Rust_(lang)"
        );
        assert_eq!(
            address("https://example.com/docs>, then"),
            "https://example.com/docs"
        );
        assert_eq!(
            address("https://example.com/a), more"),
            "https://example.com/a"
        );
        assert_eq!(
            links,
            [(4..19, "https://www.example.com/a/b?c=1".to_owned())]
        );
    }
}
