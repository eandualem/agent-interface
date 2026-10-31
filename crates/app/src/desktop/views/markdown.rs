//! The one Markdown reader every view uses: skills, policies, reports, answers
//! and conversation messages. Authored messages keep leading rules as content.
//! CommonMark with tables, strikethrough and task lists is parsed by
//! `pulldown-cmark` into blocks, drawn in the document styles. Web addresses
//! read as their site and open when clicked; it loads no images.

use std::ops::Range;

use gpui::{
    Context, FontStyle, FontWeight, HighlightStyle, StrikethroughStyle, div, prelude::*, px,
};
use pulldown_cmark::{Event, LinkType, Options, Parser, Tag, TagEnd, TextMergeStream};

use super::selection::{self, Selectable};

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

/// An inline style over part of a run of text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Mark {
    Code,
    Strong,
    Emphasis,
    Strike,
}

impl Mark {
    /// Add this mark's look to `style`.
    pub fn apply(self, style: &mut HighlightStyle) {
        let c = colors();
        match self {
            Self::Code => {
                style.color = Some(c.code.into());
                style.background_color = Some(c.selected.into());
            }
            Self::Strong => style.font_weight = Some(FontWeight::SEMIBOLD),
            Self::Emphasis => style.font_style = Some(FontStyle::Italic),
            Self::Strike => {
                style.strikethrough = Some(StrikethroughStyle {
                    thickness: px(1.),
                    color: None,
                })
            }
        }
    }
}

/// A displayed link and whether its text abbreviates the destination.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Link {
    pub range: Range<usize>,
    pub url: String,
    /// Restore shortened addresses on copy; preserve authored labels.
    pub copy_url: bool,
}

type Links = Vec<Link>;

// Bound the tree used by parsing, native rendering and recursive destruction.
const MAX_NESTING: usize = 32;

/// A run of text with its inline styles and links.
#[derive(Debug, Default, PartialEq)]
pub(super) struct Inline {
    pub text: String,
    pub marks: Vec<(Range<usize>, Mark)>,
    pub links: Links,
}

#[derive(Debug, PartialEq)]
pub(super) enum Block {
    Heading(u8, Inline),
    Paragraph(Inline),
    /// Each item's blocks; numbered from `start` when ordered.
    List {
        start: Option<u64>,
        items: Vec<Vec<Block>>,
    },
    Quote(Vec<Block>),
    Code(String),
    Rule,
    Table {
        head: Vec<Inline>,
        rows: Vec<Vec<Inline>>,
    },
}

/// The blocks of a document, after any front matter between `---` lines.
pub(super) fn parse(text: &str) -> Vec<Block> {
    parse_with(text, true)
}

/// Authored records contain no front matter; a leading `---` is a rule.
pub(super) fn parse_authored(text: &str) -> Vec<Block> {
    parse_with(text, false)
}

fn parse_with(text: &str, front_matter: bool) -> Vec<Block> {
    let mut body = text;
    if front_matter && text.lines().next().is_some_and(|line| line.trim() == "---") {
        let mut lines = text.split_inclusive('\n');
        let mut end = lines.next().map_or(0, str::len);
        for line in lines {
            end += line.len();
            if line.trim() == "---" {
                body = &text[end..];
                break;
            }
        }
    }
    let options =
        Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    let mut depth = 0;
    let mut too_deep = false;
    let events = Parser::new_ext(body, options).take_while(|event| {
        match event {
            Event::Start(_) => depth += 1,
            Event::End(_) => depth -= 1,
            _ => {}
        }
        too_deep = depth > MAX_NESTING;
        !too_deep
    });
    // Unmatched Markdown delimiters can split a bare URL across adjacent text
    // events. Detect its address only after those fragments are joined.
    let parsed = blocks(&mut TextMergeStream::new(events));
    if too_deep {
        // Keep all source text readable and copyable without constructing a
        // deeply nested native element tree, including for partially read blocks.
        vec![Block::Code(body.to_owned())]
    } else {
        parsed
    }
}

/// The blocks until the end of the enclosing container.
fn blocks<'a>(events: &mut impl Iterator<Item = Event<'a>>) -> Vec<Block> {
    let mut out = Vec::new();
    // Text directly in a container, as in a tight list item.
    let mut loose = Builder::default();
    while let Some(event) = events.next() {
        let block = match event {
            Event::End(end) if !inline_end(end) => break,
            Event::Start(Tag::Paragraph) => Block::Paragraph(inline(events)),
            Event::Start(Tag::Heading { level, .. }) => Block::Heading(level as u8, inline(events)),
            Event::Start(Tag::BlockQuote(_)) => Block::Quote(blocks(events)),
            Event::Start(Tag::CodeBlock(_)) => {
                let mut code = String::new();
                for event in events.by_ref() {
                    match event {
                        Event::Text(text) => code.push_str(&text),
                        Event::End(_) => break,
                        _ => {}
                    }
                }
                Block::Code(code.trim_end_matches('\n').to_owned())
            }
            Event::Start(Tag::List(start)) => {
                let mut items = Vec::new();
                while let Some(Event::Start(Tag::Item)) = events.next() {
                    items.push(blocks(events));
                }
                Block::List { start, items }
            }
            Event::Start(Tag::Table(_)) => table(events),
            Event::Start(Tag::HtmlBlock) => {
                let mut html = String::new();
                for event in events.by_ref() {
                    match event {
                        Event::Html(text) => html.push_str(&text),
                        Event::End(_) => break,
                        _ => {}
                    }
                }
                // Comments are not part of the reading.
                let html = html.trim();
                if html.is_empty() || html.starts_with("<!--") || !html.starts_with('<') {
                    continue;
                }
                Block::Code(html.to_owned())
            }
            Event::Rule => Block::Rule,
            event => {
                loose.event(event);
                continue;
            }
        };
        if !loose.inline.text.is_empty() {
            out.push(Block::Paragraph(std::mem::take(&mut loose).finish()));
        }
        out.push(block);
    }
    if !loose.inline.text.is_empty() {
        out.push(Block::Paragraph(loose.finish()));
    }
    out
}

fn table<'a>(events: &mut impl Iterator<Item = Event<'a>>) -> Block {
    let (mut head, mut rows, mut row) = (Vec::new(), Vec::new(), Vec::new());
    while let Some(event) = events.next() {
        match event {
            Event::Start(Tag::TableCell) => row.push(inline(events)),
            Event::End(TagEnd::TableHead) => head = std::mem::take(&mut row),
            Event::End(TagEnd::TableRow) => rows.push(std::mem::take(&mut row)),
            Event::End(TagEnd::Table) => break,
            _ => {}
        }
    }
    Block::Table { head, rows }
}

/// The text of a paragraph, heading or table cell, to its end.
fn inline<'a>(events: &mut impl Iterator<Item = Event<'a>>) -> Inline {
    let mut builder = Builder::default();
    for event in events.by_ref() {
        if !builder.event(event) {
            break;
        }
    }
    builder.finish()
}

/// Whether `end` closes a style or link within text, not a block.
fn inline_end(end: TagEnd) -> bool {
    matches!(
        end,
        TagEnd::Strong
            | TagEnd::Emphasis
            | TagEnd::Strikethrough
            | TagEnd::Link
            | TagEnd::Image
            | TagEnd::Superscript
            | TagEnd::Subscript
    )
}

struct OpenLink {
    url: String,
    autolink: bool,
}

#[derive(Default)]
struct Builder {
    inline: Inline,
    /// Marks and links not yet closed, with where each began.
    open: Vec<(usize, Option<Mark>, Option<OpenLink>)>,
}

impl Builder {
    /// Add an inline event. Returns false at the end of the enclosing block.
    fn event(&mut self, event: Event) -> bool {
        let text = &mut self.inline.text;
        match event {
            Event::Text(part) => {
                if self
                    .open
                    .iter()
                    .any(|(_, _, link)| link.as_ref().is_some_and(|link| !link.autolink))
                {
                    text.push_str(&part);
                } else {
                    shorten(&part, text, &mut self.inline.links);
                }
            }
            Event::Code(code) => {
                let start = text.len();
                text.push_str(&code);
                self.inline.marks.push((start..text.len(), Mark::Code));
            }
            Event::SoftBreak => text.push(' '),
            Event::HardBreak => text.push('\n'),
            Event::TaskListMarker(done) => text.push_str(if done { "☑ " } else { "☐ " }),
            Event::InlineHtml(html) | Event::Html(html) => {
                if html.eq_ignore_ascii_case("<br>") || html.eq_ignore_ascii_case("<br/>") {
                    text.push('\n');
                } else if !html.starts_with('<') {
                    text.push_str(&html);
                }
            }
            Event::InlineMath(math) | Event::DisplayMath(math) => text.push_str(&math),
            Event::FootnoteReference(name) => text.push_str(&format!("[{name}]")),
            Event::Start(tag) => {
                let start = text.len();
                match tag {
                    Tag::Strong => self.open.push((start, Some(Mark::Strong), None)),
                    Tag::Emphasis => self.open.push((start, Some(Mark::Emphasis), None)),
                    Tag::Strikethrough => self.open.push((start, Some(Mark::Strike), None)),
                    Tag::Link {
                        dest_url,
                        link_type,
                        ..
                    } => {
                        self.open.push((
                            start,
                            None,
                            Some(OpenLink {
                                url: dest_url.into_string(),
                                autolink: link_type == LinkType::Autolink,
                            }),
                        ));
                    }
                    Tag::Image { dest_url, .. } => {
                        self.open.push((
                            start,
                            None,
                            Some(OpenLink {
                                url: dest_url.into_string(),
                                autolink: false,
                            }),
                        ));
                    }
                    _ => self.open.push((start, None, None)),
                }
            }
            Event::End(end) if inline_end(end) => {
                let Some((start, mark, url)) = self.open.pop() else {
                    return true;
                };
                if let Some(mark) = mark {
                    self.inline.marks.push((start..text.len(), mark));
                }
                if let Some(OpenLink { url, autolink }) = url {
                    self.link(start, url, autolink);
                }
            }
            Event::End(_) => return false,
            Event::Rule => {}
        }
        true
    }

    /// Mark `start..` as a link to `url`. A label that is the address itself,
    /// already shortened to its site, keeps that link.
    fn link(&mut self, start: usize, url: String, autolink: bool) {
        let links = &mut self.inline.links;
        // Links arrive in display order. Only auto-links within this label can
        // need replacement; previously completed links must not be rescanned.
        while links.last().is_some_and(|link| link.range.start >= start) {
            links.pop();
        }
        let url = url.trim().to_owned();
        let web = url.split_once("://").is_some_and(|(scheme, _)| {
            scheme.eq_ignore_ascii_case("https") || scheme.eq_ignore_ascii_case("http")
        });
        if !web {
            if self.inline.text.len() == start {
                self.inline.text.push_str(&url);
            }
            return;
        }
        if self.inline.text.len() == start {
            // A link with no label reads as its site.
            shorten(&url, &mut self.inline.text, links);
            return;
        }
        links.push(Link {
            range: start..self.inline.text.len(),
            url,
            copy_url: autolink,
        });
    }

    fn finish(mut self) -> Inline {
        self.inline.marks.sort_by_key(|(range, _)| range.start);
        self.inline
    }
}

/// The web address at the start of `text`. It ends at a space or an angle
/// bracket, as in `<https://…>`; closing punctuation belongs to the sentence,
/// and a closing parenthesis or bracket too unless the address opened it, as in
/// `https://en.wikipedia.org/wiki/Rust_(programming_language)`.
fn address(text: &str) -> &str {
    let end = text
        .find(|ch: char| ch.is_whitespace() || ch == '<' || ch == '>')
        .unwrap_or(text.len());
    let mut url = &text[..end];
    let balance = |open, close| {
        url.bytes().fold(0isize, |count, byte| {
            count + isize::from(byte == open) - isize::from(byte == close)
        })
    };
    let mut round = balance(b'(', b')');
    let mut square = balance(b'[', b']');
    loop {
        url = url.trim_end_matches(['.', ',', ';', ':', '!', '?', '\'', '"']);
        if url.ends_with(')') && round < 0 {
            round += 1;
        } else if url.ends_with(']') && square < 0 {
            square += 1;
        } else {
            return url;
        }
        url = &url[..url.len() - 1];
    }
}

/// Append `text` to `plain`, each address in it as its site.
fn shorten(mut text: &str, plain: &mut String, links: &mut Links) {
    // Search the common prefix once. Searching the entire remaining suffix for
    // each scheme separately is quadratic when many URLs use only one scheme.
    while let Some(at) = text.find("http") {
        let rest = &text[at..];
        if !rest.starts_with("https://") && !rest.starts_with("http://") {
            plain.push_str(&text[..at + 4]);
            text = &text[at + 4..];
            continue;
        }
        plain.push_str(&text[..at]);
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
            links.push(Link {
                range: start..plain.len(),
                url: url.to_owned(),
                copy_url: true,
            });
        }
        text = &rest[url.len()..];
    }
    plain.push_str(text);
}

fn styled<V: Selectable>(inline: &Inline, pane: &V, cx: &mut Context<V>) -> impl IntoElement {
    selection::linked(pane, &inline.text, &inline.marks, &inline.links, cx)
}

/// Draw the blocks as a document.
pub(super) fn render<V: Selectable>(blocks: &[Block], pane: &V, cx: &mut Context<V>) -> gpui::Div {
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
pub(super) fn render_in<V: Selectable>(
    blocks: &[Block],
    look: &Look,
    pane: &V,
    cx: &mut Context<V>,
) -> gpui::Div {
    let document = ui::text(
        div()
            .flex()
            .flex_col()
            .cursor_text()
            .text_color(look.color)
            .line_height(gpui::relative(look.line_height)),
        look.style,
    );
    selection::area(document, cx).child(column(blocks, look.gap, pane, cx))
}

/// `blocks` one under another, `gap` apart.
fn column<V: Selectable>(blocks: &[Block], gap: f32, pane: &V, cx: &mut Context<V>) -> gpui::Div {
    let (t, c) = (tokens(), colors());
    let mut out = div().flex().flex_col().gap(px(gap)).min_w_0();
    for (at, block) in blocks.iter().enumerate() {
        out = out.child(match block {
            Block::Heading(level, text) => {
                let style = if *level == 1 {
                    &t.text.doc_title
                } else {
                    &t.text.doc_heading
                };
                ui::text(div().text_color(c.text), style)
                    // Headings after the first stand apart from what precedes them.
                    .when(at > 0 && *level > 1, |el| el.mt(px(t.space.s8)))
                    .when(*level > 2, |el| el.text_size(t.text.doc_body.size()))
                    .child(styled(text, pane, cx))
                    .into_any_element()
            }
            Block::Paragraph(text) => div().child(styled(text, pane, cx)).into_any_element(),
            Block::List { start, items } => {
                let mut list = div()
                    .flex()
                    .flex_col()
                    .gap(px(t.space.s4))
                    .pl(px(t.space.s4));
                for (index, item) in items.iter().enumerate() {
                    let marker = match start {
                        Some(first) => format!("{}.", first + index as u64),
                        None => "•".into(),
                    };
                    list = list.child(
                        div()
                            .flex()
                            .gap(px(t.space.s8))
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .min_w(px(t.space.s14))
                                    .text_color(c.muted)
                                    .child(marker),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .child(column(item, t.space.s6, pane, cx)),
                            ),
                    );
                }
                list.into_any_element()
            }
            Block::Quote(blocks) => div()
                .pl(px(t.space.s12))
                .border_l(px(t.space.s2))
                .border_color(c.line_strong)
                .text_color(c.muted)
                .child(column(blocks, gap, pane, cx))
                .into_any_element(),
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
                    .line_height(gpui::relative(1.5))
                    .overflow_hidden(),
                &t.text.status,
            )
            .child(selection::text(pane, code, &[], cx))
            .into_any_element(),
            Block::Rule => div()
                .my(px(t.space.s4))
                .h(px(t.size.hairline))
                .bg(c.line)
                .into_any_element(),
            Block::Table { head, rows } => {
                let row = |cells: &[Inline], header: bool, cx: &mut Context<V>| {
                    let mut line = div()
                        .flex()
                        .border_t(px(t.size.hairline))
                        .border_color(c.line)
                        .when(header, |el| el.border_t_0().bg(c.field).text_color(c.text));
                    for cell in cells {
                        line = line.child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .px(px(t.space.s10))
                                .py(px(t.space.s6))
                                .when(header, |el| el.font_weight(FontWeight::SEMIBOLD))
                                .child(styled(cell, pane, cx)),
                        );
                    }
                    line
                };
                let mut table = div()
                    .flex()
                    .flex_col()
                    .rounded(px(t.radius.slot))
                    .border(px(t.size.hairline))
                    .border_color(c.line)
                    .overflow_hidden();
                if !head.is_empty() {
                    table = table.child(row(head, true, cx));
                }
                for cells in rows {
                    table = table.child(row(cells, false, cx));
                }
                table.into_any_element()
            }
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(text: &str) -> Inline {
        Inline {
            text: text.into(),
            ..Inline::default()
        }
    }

    #[test]
    fn authored_rules_do_not_hide_the_text_between_them() {
        assert_eq!(
            parse_authored("---\n\nKeep this answer\n\n---\nAfter"),
            [
                Block::Rule,
                Block::Paragraph(plain("Keep this answer")),
                Block::Rule,
                Block::Paragraph(plain("After")),
            ]
        );
    }

    #[test]
    fn extreme_nesting_falls_back_to_complete_source_text() {
        let source = format!("{}x", "> ".repeat(100_000));
        assert!(source.len() < 256 * 1024);
        assert_eq!(parse_authored(&source), [Block::Code(source)]);
        // Normal nesting still renders as Markdown, not the fallback.
        assert!(matches!(
            parse_authored("> > **nested**").first(),
            Some(Block::Quote(_))
        ));
    }

    #[test]
    fn explicit_links_preserve_their_authored_label() {
        let blocks = parse_authored("[first](https://first) [https://a https://b](https://target)");
        let Block::Paragraph(inline) = &blocks[0] else {
            panic!()
        };
        assert_eq!(inline.text, "first https://a https://b");
        assert_eq!(inline.links.len(), 2);
        assert_eq!(
            inline.links[0],
            Link {
                range: 0..5,
                url: "https://first".into(),
                copy_url: false
            }
        );
        assert_eq!(
            inline.links[1],
            Link {
                range: 6..inline.text.len(),
                url: "https://target".into(),
                copy_url: false
            }
        );
    }

    #[test]
    fn large_link_paragraphs_keep_every_destination() {
        for item in ["[x](https://a) ", "https://a ", "http://a "] {
            let source = item.repeat(16_000);
            assert!(source.len() < 256 * 1024);
            let blocks = parse_authored(&source);
            let Block::Paragraph(inline) = &blocks[0] else {
                panic!()
            };
            assert_eq!(inline.links.len(), 16_000);
            let destination = if item.starts_with("http://") {
                "http://a"
            } else {
                "https://a"
            };
            assert!(inline.links.iter().all(|link| link.url == destination));
        }
    }

    #[test]
    fn url_scanning_preserves_prefix_text_and_trims_long_unbalanced_suffixes() {
        let mut text = String::new();
        let mut links = Vec::new();
        shorten("httpx httphotos httphttp://a tail", &mut text, &mut links);
        assert_eq!(text, "httpx httphotos httpa ↗ tail");
        assert_eq!(links[0].url, "http://a");
        let source = format!("https://example.com/[{}", "]".repeat(100_000));
        assert_eq!(address(&source), "https://example.com/[]");
    }

    #[test]
    fn document_rules_only_strip_metadata_at_the_beginning() {
        let body = "# Guide\n\n---\nImportant warning\n\n---\nAfter";
        let expected = parse_authored(body);
        assert_eq!(parse(body), expected);
        assert_eq!(parse(&format!("---\nname: guide\n---\n{body}")), expected);
        assert_eq!(parse("---\nUnclosed"), parse_authored("---\nUnclosed"));
    }

    #[test]
    fn documents_split_into_blocks_without_front_matter() {
        let text = "---\nname: check\n---\n# Check\n\nRun it `before` pushing,\nalways.\n\n## When\n- one\n- two\n  more\n\n3. three\n\n```bash\nrun --now\n```\nafter";
        let blocks = parse(text);
        assert_eq!(blocks[0], Block::Heading(1, plain("Check")));
        assert_eq!(
            blocks[1],
            Block::Paragraph(Inline {
                text: "Run it before pushing, always.".into(),
                marks: vec![(7..13, Mark::Code)],
                links: vec![],
            })
        );
        assert_eq!(
            blocks[3],
            Block::List {
                start: None,
                items: vec![
                    vec![Block::Paragraph(plain("one"))],
                    vec![Block::Paragraph(plain("two more"))]
                ],
            }
        );
        assert_eq!(
            blocks[4],
            Block::List {
                start: Some(3),
                items: vec![vec![Block::Paragraph(plain("three"))]],
            }
        );
        assert_eq!(blocks[5], Block::Code("run --now".into()));
        assert_eq!(blocks[6], Block::Paragraph(plain("after")));
    }

    #[test]
    fn inline_styles_links_and_nesting_are_kept() {
        let blocks = parse_authored(
            "- **Talk to agents**: run `tell` *now* ~~later~~, see [the guide](https://x.com/g)\n  - nested\n\n> quoted\n\n| a | b |\n|---|---|\n| 1 | 2 |",
        );
        let Block::List { items, .. } = &blocks[0] else {
            panic!("{blocks:?}")
        };
        let Block::Paragraph(first) = &items[0][0] else {
            panic!()
        };
        assert_eq!(
            first.text,
            "Talk to agents: run tell now later, see the guide"
        );
        assert_eq!(
            first.marks,
            [
                (0..14, Mark::Strong),
                (20..24, Mark::Code),
                (25..28, Mark::Emphasis),
                (29..34, Mark::Strike),
            ]
        );
        assert_eq!(
            first.links,
            [Link {
                range: 40..49,
                url: "https://x.com/g".to_owned(),
                copy_url: false
            }]
        );
        assert!(matches!(&items[0][1], Block::List { items, .. } if items.len() == 1));
        assert_eq!(
            blocks[1],
            Block::Quote(vec![Block::Paragraph(plain("quoted"))])
        );
        assert_eq!(
            blocks[2],
            Block::Table {
                head: vec![plain("a"), plain("b")],
                rows: vec![vec![plain("1"), plain("2")]],
            }
        );
    }

    #[test]
    fn explicit_links_only_open_web_destinations() {
        let blocks = parse_authored(
            "[file](file:///example) [shell](ssh://example) [anchor](#part) [](custom:open) [web](HTTPS://example.com)",
        );
        let Block::Paragraph(inline) = &blocks[0] else {
            panic!()
        };
        assert_eq!(inline.text, "file shell anchor custom:open web");
        assert_eq!(
            inline.links,
            [Link {
                range: 30..33,
                url: "HTTPS://example.com".to_owned(),
                copy_url: false
            }]
        );
    }

    #[test]
    fn bare_url_keeps_delimiters_split_across_text_events() {
        let blocks = parse_authored("See https://example.com/~alice");
        let Block::Paragraph(inline) = &blocks[0] else {
            panic!()
        };
        assert_eq!(inline.text, "See example.com ↗");
        assert_eq!(
            inline.links,
            [Link {
                range: 4..19,
                url: "https://example.com/~alice".to_owned(),
                copy_url: true
            }]
        );
    }

    #[test]
    fn web_addresses_read_as_their_site() {
        let blocks = parse_authored(
            "See https://www.example.com/a/b?c=1. Run `https://x.y` too <https://z.org/q>",
        );
        let Block::Paragraph(inline) = &blocks[0] else {
            panic!()
        };
        assert_eq!(
            inline.text,
            "See example.com ↗. Run https://x.y too z.org ↗"
        );
        assert_eq!(inline.marks, [(25..36, Mark::Code)]);
        assert_eq!(
            inline.links,
            [
                Link {
                    range: 4..19,
                    url: "https://www.example.com/a/b?c=1".to_owned(),
                    copy_url: true
                },
                Link {
                    range: 41..50,
                    url: "https://z.org/q".to_owned(),
                    copy_url: true
                }
            ]
        );
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
        assert_eq!(address("https://example.com/b]"), "https://example.com/b");
        assert_eq!(
            address("https://example.com/[x]"),
            "https://example.com/[x]"
        );
    }
}
