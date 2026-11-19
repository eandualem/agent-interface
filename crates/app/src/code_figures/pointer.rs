//! Diff and code pointers in a Document's HTML, found as a browser tokenizes
//! the page, with the byte ranges a view replaces. Anything malformed is not
//! a pointer and stays as authored.
//!
//! ```html
//! <figure data-backbone-diff="1" data-repo="owner/name"
//!         data-base="<40 hex>" data-head="<40 hex>" data-paths='["src/a.rs"]'>
//!   <figcaption>optional caption</figcaption>
//! </figure>
//! <figure data-backbone-code="1" data-repo="owner/name"
//!         data-commit="<40 hex>" data-path="src/a.rs" data-lines="40-58"></figure>
//! ```

use std::{
    cell::{Cell, RefCell},
    ops::Range,
};

use html5ever::{
    LocalName,
    tendril::StrTendril,
    tokenizer::{
        BufferQueue, Tag, TagKind, Token, TokenSink, TokenSinkResult, Tokenizer, TokenizerOpts,
        states::RawKind,
    },
};

use super::Until;

/// The most pointers read from one Document; later ones stay as authored.
pub const LIMIT: usize = 64;
/// The most bytes fed to the tokenizer at once. The tokenizer compares each
/// new attribute of a tag with all before it, so a step's cost grows with
/// the attributes already in its tag; at this size the slowest step measured
/// late in a 4 MiB tag of distinct attributes took about 0.11 s.
const STEP: usize = 1024;
/// The most paths a diff pointer may name.
const PATHS: usize = 100;

/// What a pointer asks Git for. Commits are 40 lowercase hexadecimal digits.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Pointer {
    /// `git diff BASE HEAD` of the two commits' trees, limited to `paths`.
    Diff {
        /// GitHub `owner/name`, as authored.
        repo: String,
        /// The old commit.
        base: String,
        /// The new commit.
        head: String,
        /// Repository-relative paths; none means the whole diff.
        paths: Option<Vec<String>>,
    },
    /// The file `path` at `commit`.
    Code {
        /// GitHub `owner/name`, as authored.
        repo: String,
        /// The commit.
        commit: String,
        /// A repository-relative path.
        path: String,
        /// First and last line, 1-based and inclusive; none means the whole file.
        lines: Option<(u32, u32)>,
    },
}

impl Pointer {
    /// The GitHub `owner/name` it points into.
    pub fn repo(&self) -> &str {
        match self {
            Self::Diff { repo, .. } | Self::Code { repo, .. } => repo,
        }
    }
}

/// One pointer and where it sits in the page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    /// What it asks for.
    pub pointer: Pointer,
    /// The figure's content, between its start and end tags: what a view replaces.
    pub content: Range<usize>,
    /// The content of its figcaption, exactly as authored.
    pub caption: Option<Range<usize>>,
}

/// The pointers in `html`, in page order, at most [`LIMIT`]. A pointer is a
/// `figure` in HTML content with valid pointer attributes, holding only
/// whitespace, comments and at most one `figcaption`, closed by `</figure>`.
/// None are found once `until` passes.
pub fn find(html: &str, until: &Until) -> Vec<Found> {
    let tokenizer = Tokenizer::new(
        Sink::default(),
        TokenizerOpts {
            // Every chunk below is a separate feed; a BOM is only text here.
            discard_bom: false,
            ..TokenizerOpts::default()
        },
    );
    let queue = BufferQueue::default();
    let mut scan = Scan {
        html,
        markup_end: 0,
        open: None,
        found: Vec::new(),
    };
    // The tokenizer reports no byte offsets, but every tag ends at a `>` and
    // is emitted when that `>` is consumed. Feeding the page in chunks that
    // each end at a `>` places the end of every tag, comment and doctype
    // exactly at the end of the chunk that emitted it.
    let mut start = 0;
    while start < html.len() && scan.found.len() < LIMIT {
        let end = html[start..]
            .find('>')
            .map_or(html.len(), |at| start + at + 1);
        // In steps, so that even a tag that is slow to read holds the scan at
        // most one step past `until`.
        let mut from = start;
        while from < end {
            if until.passed() {
                return Vec::new();
            }
            // At most a step: back to the start of a character it splits.
            let mut to = (from + STEP).min(end);
            while !html.is_char_boundary(to) {
                to -= 1;
            }
            queue.push_back(StrTendril::from_slice(&html[from..to]));
            let _ = tokenizer.feed(&queue);
            from = to;
        }
        if until.passed() {
            return Vec::new();
        }
        for event in tokenizer.sink.events.take() {
            scan.event(event, end);
        }
        start = end;
    }
    // A figure still open at the end is unclosed and stays as authored.
    scan.found
}

/// `html` with each range replaced by its view. Ranges come from [`find`]:
/// in order, apart and on character boundaries.
pub fn splice(html: &str, views: &[(Range<usize>, String)]) -> String {
    let added: usize = views.iter().map(|(_, view)| view.len()).sum();
    let mut out = String::with_capacity(html.len() + added);
    let mut at = 0;
    for (range, view) in views {
        debug_assert!(at <= range.start && range.start <= range.end);
        out.push_str(&html[at..range.start]);
        out.push_str(view);
        at = range.end;
    }
    out.push_str(&html[at..]);
    out
}

enum Event {
    /// A start tag, and whether it is in HTML content rather than SVG or MathML.
    Start(Tag, bool),
    End(LocalName),
    /// A comment or doctype.
    Comment,
    /// Text, and whether it is only whitespace.
    Text(bool),
}

/// Collects tokens and switches the tokenizer into the text states a browser's
/// tree builder would, so markup inside a script, style or textarea is text.
#[derive(Default)]
struct Sink {
    events: RefCell<Vec<Event>>,
    /// Open `svg` and `math` elements, where these elements are not raw text.
    foreign: Cell<u32>,
}

impl TokenSink for Sink {
    type Handle = ();

    fn process_token(&self, token: Token, _line: u64) -> TokenSinkResult<()> {
        let mut result = TokenSinkResult::Continue;
        let event = match token {
            Token::TagToken(tag) => {
                let foreign = self.foreign.get();
                let name = &*tag.name;
                match tag.kind {
                    TagKind::StartTag => {
                        if foreign == 0 {
                            result = match name {
                                "script" => TokenSinkResult::RawData(RawKind::ScriptData),
                                "style" | "xmp" | "iframe" | "noembed" | "noframes"
                                | "noscript" => TokenSinkResult::RawData(RawKind::Rawtext),
                                "textarea" | "title" => TokenSinkResult::RawData(RawKind::Rcdata),
                                "plaintext" => TokenSinkResult::Plaintext,
                                _ => TokenSinkResult::Continue,
                            };
                        }
                        if matches!(name, "svg" | "math") && !tag.self_closing {
                            self.foreign.set(foreign + 1);
                        }
                        Event::Start(tag, foreign == 0)
                    }
                    TagKind::EndTag => {
                        if foreign > 0 && matches!(name, "svg" | "math") {
                            self.foreign.set(foreign - 1);
                        }
                        // SVG and MathML have no such elements, so a browser
                        // closes any of their content still open here.
                        if matches!(name, "figure" | "figcaption") {
                            self.foreign.set(0);
                        }
                        Event::End(tag.name)
                    }
                }
            }
            Token::CommentToken(_) | Token::DoctypeToken(_) => Event::Comment,
            Token::CharacterTokens(text) => Event::Text(text.chars().all(is_space)),
            Token::NullCharacterToken => Event::Text(false),
            Token::EOFToken | Token::ParseError(_) => return result,
        };
        let mut events = self.events.borrow_mut();
        // A run of text is one event, however many tokens it took.
        if let (Some(Event::Text(blank)), Event::Text(also)) = (events.last_mut(), &event) {
            *blank &= *also;
        } else {
            events.push(event);
        }
        result
    }

    fn adjusted_current_node_present_but_not_in_html_namespace(&self) -> bool {
        self.foreign.get() > 0
    }
}

/// HTML whitespace.
fn is_space(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\x0c' | '\r' | ' ')
}

struct Scan<'a> {
    html: &'a str,
    /// Where the last tag, comment or doctype ended. Only text lies between
    /// it and the next one, so a tag's start is the first `<name` after it.
    markup_end: usize,
    open: Option<Open>,
    found: Vec<Found>,
}

/// A pointer figure whose end tag has not been reached.
struct Open {
    pointer: Pointer,
    content: usize,
    caption: Caption,
}

enum Caption {
    Before,
    Inside(usize),
    After(Range<usize>),
}

/// What one event does to an open pointer figure.
enum Step {
    Open(Open),
    Closed,
    /// Not a pointer after all.
    Rejected,
}

impl Scan<'_> {
    /// Take one event; a tag, comment or doctype ends at `end`.
    fn event(&mut self, event: Event, end: usize) {
        if let Some(open) = self.open.take() {
            match self.step(open, &event, end) {
                Step::Open(open) => self.open = Some(open),
                Step::Closed => {}
                // The event that ended it may itself begin a pointer.
                Step::Rejected => {
                    self.event(event, end);
                    return;
                }
            }
        } else if let Event::Start(tag, true) = &event
            && &*tag.name == "figure"
            && self.found.len() < LIMIT
            && let Some(pointer) = pointer(tag)
        {
            self.open = Some(Open {
                pointer,
                content: end,
                caption: Caption::Before,
            });
        }
        if !matches!(event, Event::Text(_)) {
            self.markup_end = end;
        }
    }

    fn step(&mut self, mut open: Open, event: &Event, end: usize) -> Step {
        match (&open.caption, event) {
            (_, Event::Comment | Event::Text(true)) | (Caption::Inside(_), Event::Text(_)) => {
                Step::Open(open)
            }
            (Caption::Before, Event::Start(tag, _)) if &*tag.name == "figcaption" => {
                open.caption = Caption::Inside(end);
                Step::Open(open)
            }
            // The caption becomes a view's summary, so it may not open or
            // close a summary or details of its own, nor open SVG or MathML,
            // whose content the tokenizer reads differently.
            (Caption::Inside(_), Event::Start(tag, _))
                if !matches!(
                    &*tag.name,
                    "figure" | "figcaption" | "details" | "summary" | "svg" | "math"
                ) =>
            {
                Step::Open(open)
            }
            (&Caption::Inside(start), Event::End(name)) if &**name == "figcaption" => {
                match tag_start(self.html, self.markup_end, end, "</figcaption") {
                    Some(stop) => {
                        open.caption = Caption::After(start..stop);
                        Step::Open(open)
                    }
                    None => Step::Rejected,
                }
            }
            (Caption::Inside(_), Event::End(name))
                if !matches!(&**name, "figure" | "details" | "summary") =>
            {
                Step::Open(open)
            }
            (Caption::Before | Caption::After(_), Event::End(name)) if &**name == "figure" => {
                let Some(stop) = tag_start(self.html, self.markup_end, end, "</figure") else {
                    return Step::Rejected;
                };
                self.found.push(Found {
                    pointer: open.pointer,
                    content: open.content..stop,
                    caption: match open.caption {
                        Caption::After(caption) => Some(caption),
                        _ => None,
                    },
                });
                Step::Closed
            }
            _ => Step::Rejected,
        }
    }
}

/// Where the tag `name` (such as `</figure`) begins in `html[from..to]`,
/// matched as the tokenizer does: ASCII case-insensitive, then whitespace,
/// `/` or `>`.
fn tag_start(html: &str, from: usize, to: usize, name: &str) -> Option<usize> {
    let bytes = html.as_bytes();
    let name = name.as_bytes();
    (from..to).find(|&at| {
        bytes
            .get(at..at + name.len())
            .is_some_and(|word| word.eq_ignore_ascii_case(name))
            && bytes.get(at + name.len()).is_some_and(|next| {
                matches!(next, b'\t' | b'\n' | b'\x0c' | b'\r' | b' ' | b'/' | b'>')
            })
    })
}

/// The pointer a figure's attributes describe, if they are valid.
fn pointer(tag: &Tag) -> Option<Pointer> {
    // A browser keeps the first of a repeated attribute; a pointer must not
    // depend on that.
    if tag.had_duplicate_attributes {
        return None;
    }
    let value = |name: &str| {
        tag.attrs
            .iter()
            .find(|attribute| &*attribute.name.local == name)
            .map(|attribute| &*attribute.value)
    };
    match (value("data-backbone-diff"), value("data-backbone-code")) {
        (Some("1"), None) => Some(Pointer::Diff {
            repo: repo(value("data-repo")?)?,
            base: commit(value("data-base")?)?,
            head: commit(value("data-head")?)?,
            paths: match value("data-paths") {
                Some(json) => Some(paths(json)?),
                None => None,
            },
        }),
        (None, Some("1")) => Some(Pointer::Code {
            repo: repo(value("data-repo")?)?,
            commit: commit(value("data-commit")?)?,
            path: value("data-path").filter(|path| is_path(path))?.to_owned(),
            lines: match value("data-lines") {
                Some(range) => Some(lines(range)?),
                None => None,
            },
        }),
        _ => None,
    }
}

/// A GitHub `owner/name`.
fn repo(value: &str) -> Option<String> {
    let (owner, name) = value.split_once('/')?;
    let owner_ok = (1..=39).contains(&owner.len())
        && owner
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-');
    let name_ok = (1..=100).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
        && name != "."
        && name != "..";
    (owner_ok && name_ok).then(|| value.to_owned())
}

/// A full commit ID, lowercased.
fn commit(value: &str) -> Option<String> {
    (value.len() == 40 && value.bytes().all(|b| b.is_ascii_hexdigit()))
        .then(|| value.to_ascii_lowercase())
}

/// A repository-relative path: `/`-separated parts, none empty, `.` or `..`,
/// and no control characters or backslashes.
fn is_path(value: &str) -> bool {
    (1..=4096).contains(&value.len())
        && !value.chars().any(|c| c.is_control() || c == '\\')
        && value
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..")
}

/// A JSON array of one or more paths.
fn paths(json: &str) -> Option<Vec<String>> {
    let paths: Vec<String> = serde_json::from_str(json).ok()?;
    let ok = (1..=PATHS).contains(&paths.len()) && paths.iter().all(|path| is_path(path));
    ok.then_some(paths)
}

/// `N` or `N-M`, 1-based and inclusive, with M at least N.
fn lines(value: &str) -> Option<(u32, u32)> {
    let number = |text: &str| -> Option<u32> {
        if text.is_empty() || text.len() > 9 || !text.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        text.parse().ok().filter(|&number| number >= 1)
    };
    let (first, last) = match value.split_once('-') {
        Some((first, last)) => (number(first)?, number(last)?),
        None => (number(value)?, number(value)?),
    };
    (last >= first).then_some((first, last))
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "0123456789abcdef0123456789abcdef01234567";
    const B: &str = "89abcdef0123456789abcdef0123456789abcdef";

    fn diff(attributes: &str, content: &str) -> String {
        format!(
            r#"<figure data-backbone-diff="1" data-repo="owner/name" data-base="{A}" data-head="{B}" {attributes}>{content}</figure>"#
        )
    }

    fn code(attributes: &str) -> String {
        format!(
            r#"<figure data-backbone-code="1" data-repo="owner/name" data-commit="{A}" data-path="src/a.rs" {attributes}></figure>"#
        )
    }

    fn later() -> Until {
        Until::new(std::time::Instant::now() + std::time::Duration::from_secs(10))
    }

    fn pointers(html: &str) -> Vec<Pointer> {
        find(html, &later())
            .into_iter()
            .map(|found| found.pointer)
            .collect()
    }

    #[test]
    fn finds_both_pointers_with_their_content_and_caption() {
        let first = diff(
            r#"data-paths='["src/a.rs","b c/d.md"]'"#,
            "\n  <figcaption>The <em>fix</em></figcaption>\n",
        );
        let html = format!(
            "<p>é before</p>{first}<p>between</p>{}é",
            code(r#"data-lines="40-58""#)
        );
        let found = find(&html, &later());
        assert_eq!(found.len(), 2);
        assert_eq!(
            found[0].pointer,
            Pointer::Diff {
                repo: "owner/name".into(),
                base: A.into(),
                head: B.into(),
                paths: Some(vec!["src/a.rs".into(), "b c/d.md".into()]),
            }
        );
        assert_eq!(
            &html[found[0].content.clone()],
            "\n  <figcaption>The <em>fix</em></figcaption>\n"
        );
        assert_eq!(
            found[0].caption.clone().map(|range| &html[range]),
            Some("The <em>fix</em>")
        );
        assert_eq!(
            found[1].pointer,
            Pointer::Code {
                repo: "owner/name".into(),
                commit: A.into(),
                path: "src/a.rs".into(),
                lines: Some((40, 58)),
            }
        );
        assert_eq!(found[1].content.len(), 0);
        assert_eq!(found[1].caption, None);
        assert_eq!(found[1].pointer.repo(), "owner/name");
    }

    #[test]
    fn splice_changes_only_the_figure_content() {
        let html = format!(
            "<!doctype html><p>keep</p>{}<p>tail é</p>",
            diff(r#"class="x""#, "<figcaption>c</figcaption>")
        );
        let found = find(&html, &later());
        let range = found[0].content.clone();
        let out = splice(
            &html,
            &[(range.clone(), "<details open>v</details>".into())],
        );
        assert_eq!(&out[..range.start], &html[..range.start]);
        assert_eq!(
            &out[range.start + "<details open>v</details>".len()..],
            &html[range.end..]
        );
        assert!(out.contains(r#"class="x"><details open>v</details></figure>"#));
        assert_eq!(splice(&html, &[]), html);
    }

    #[test]
    fn attributes_are_read_as_a_browser_reads_them() {
        // Upper case, unquoted values, a `>` inside a quoted value, entities
        // and line breaks between attributes.
        let html = format!(
            "<FIGURE title=\"a > b\"\r\n DATA-BACKBONE-CODE=1 data-repo=Owner/Name.rs data-commit={} data-path=\"src/&lt;x&gt;.rs\" data-lines=7></Figure >",
            A.to_ascii_uppercase()
        );
        assert_eq!(
            pointers(&html),
            vec![Pointer::Code {
                repo: "Owner/Name.rs".into(),
                commit: A.into(),
                path: "src/<x>.rs".into(),
                lines: Some((7, 7)),
            }]
        );
        let html = diff("data-paths=\"[&quot;a.rs&quot;]\"", "");
        assert!(
            matches!(&pointers(&html)[..], [Pointer::Diff { paths: Some(paths), .. }] if paths == &["a.rs"])
        );
        assert!(matches!(
            &pointers(&diff("", ""))[..],
            [Pointer::Diff { paths: None, .. }]
        ));
        assert!(matches!(
            &pointers(&code(""))[..],
            [Pointer::Code { lines: None, .. }]
        ));
    }

    #[test]
    fn malformed_or_hostile_attributes_are_not_pointers() {
        let short = &A[..39];
        let long = format!("{A}0");
        let not_hex = A.replace('a', "g");
        let cases = [
            // Version and kind.
            diff("", "").replace(r#"diff="1""#, r#"diff="2""#),
            diff("", "").replace(r#"diff="1""#, r#"diff="01""#),
            diff("", "").replace(r#"diff="1""#, r#"diff"#),
            diff(r#"data-backbone-code="1""#, ""),
            // Commits.
            diff("", "").replace(A, short),
            diff("", "").replace(A, &long),
            diff("", "").replace(B, &not_hex),
            diff("", "").replace(&format!(r#"data-head="{B}""#), ""),
            code("").replace(A, "HEAD"),
            // Repositories.
            diff("", "").replace("owner/name", "owner/name/extra"),
            diff("", "").replace("owner/name", "../name"),
            diff("", "").replace("owner/name", "owner/.."),
            diff("", "").replace("owner/name", "owner"),
            diff("", "").replace("owner/name", "own er/name"),
            code("").replace(r#" data-repo="owner/name""#, ""),
            // Paths.
            diff(r#"data-paths='["../secret"]'"#, ""),
            diff(r#"data-paths='["/etc/passwd"]'"#, ""),
            diff(r#"data-paths='["a//b"]'"#, ""),
            diff(r#"data-paths='["./a"]'"#, ""),
            diff(r#"data-paths='["a\\b"]'"#, ""),
            diff(r#"data-paths='["a&#10;b"]'"#, ""),
            diff(r#"data-paths='["a&#1;b"]'"#, ""),
            diff(r#"data-paths='["a\u0001b"]'"#, ""),
            diff(r#"data-paths='[""]'"#, ""),
            diff(r#"data-paths='[]'"#, ""),
            diff(r#"data-paths='["a",'"#, ""),
            diff(r#"data-paths='{"a":1}'"#, ""),
            diff(r#"data-paths='[1]'"#, ""),
            diff(r#"data-paths='"a"'"#, ""),
            code("").replace("src/a.rs", "src/../a.rs"),
            code("").replace("src/a.rs", "src/"),
            code("").replace("src/a.rs", "a&#1;b"),
            code("").replace("src/a.rs", "a&#x7f;b"),
            code("").replace("src/a.rs", "a&#9;b"),
            // Lines.
            code(r#"data-lines="0""#),
            code(r#"data-lines="5-3""#),
            code(r#"data-lines="1-""#),
            code(r#"data-lines="-3""#),
            code(r#"data-lines="1-2-3""#),
            code(r#"data-lines=" 4""#),
            code(r#"data-lines="+4""#),
            code(r#"data-lines="9999999999""#),
            // A repeated attribute.
            code(r#"data-path="other.rs""#),
        ];
        for html in cases {
            assert_eq!(pointers(&html), vec![], "{html}");
        }
    }

    #[test]
    fn only_a_caption_whitespace_and_comments_may_be_inside() {
        for content in [
            "text",
            "<p>more</p>",
            "<figcaption>a</figcaption>text",
            "<figcaption>a</figcaption><figcaption>b</figcaption>",
            "<figcaption>a",
            "<figcaption>a<figcaption>b</figcaption></figcaption>",
            "<figcaption>a</summary></figcaption>",
            "<figcaption><details>a</figcaption>",
            "<img src=x>",
        ] {
            assert_eq!(pointers(&diff("", content)), vec![], "{content}");
        }
        for content in [
            "",
            " \n\t",
            "<!-- note -->",
            " <figcaption><b>a</b> &amp; <!--c--> b</figcaption> <!--d-->",
        ] {
            assert_eq!(pointers(&diff("", content)).len(), 1, "{content}");
        }
        // Unclosed at the end of the page.
        let open = diff("", "");
        assert_eq!(pointers(open.trim_end_matches("</figure>")), vec![]);
        // A pointer inside another's caption is found; the outer one is not.
        let html = diff("", &format!("<figcaption>{}</figcaption>", code("")));
        assert!(matches!(&pointers(&html)[..], [Pointer::Code { .. }]));
    }

    #[test]
    fn markup_that_a_browser_reads_as_text_holds_no_pointers() {
        let pointer = code("");
        for (open, close) in [
            ("<!--", "-->"),
            ("<div title='", "'></div>"),
            ("<script>let s = '", "';</script>"),
            ("<style>/*", "*/</style>"),
            ("<textarea>", "</textarea>"),
            ("<title>", "</title>"),
            ("<noscript>", "</noscript>"),
            ("<svg>", "</svg>"),
            ("<math>", "</math>"),
        ] {
            let html = format!("{open}{pointer}{close}");
            assert_eq!(pointers(&html), vec![], "{html}");
            // The page goes on as HTML afterwards.
            assert_eq!(pointers(&format!("{html}{pointer}")).len(), 1, "{html}");
        }
        assert_eq!(pointers(&format!("<plaintext>{pointer}")), vec![]);
        // SVG's own style element is not raw text, and a closed SVG ends.
        assert_eq!(pointers(&format!("<svg><style/></svg>{pointer}")).len(), 1);
    }

    #[test]
    fn a_tag_that_is_slow_to_read_stops_at_the_deadline() {
        let names: String = (0..100_000).map(|n| format!("a{n} ")).collect();
        let hostile = format!("<p {names}>{}", code(""));
        let started = std::time::Instant::now();
        let soon = Until::new(started + std::time::Duration::from_millis(200));
        assert_eq!(find(&hostile, &soon), vec![]);
        // One step past the deadline, with room for a slow machine.
        assert!(started.elapsed() < std::time::Duration::from_secs(3));
        // Steps keep every byte: a page split mid-character still reads.
        let wide = format!("<p title=\"x{}\">{}", "é".repeat(STEP), code(""));
        assert_eq!(find(&wide, &later()).len(), 1);
        // A cancelled scan finds nothing.
        let until = later();
        until.cancel();
        assert_eq!(find(&code(""), &until), vec![]);
    }

    #[test]
    fn a_run_of_text_is_one_event() {
        let sink = Sink::default();
        for _ in 0..10_000 {
            let _ = sink.process_token(Token::CharacterTokens(" ".into()), 1);
            let _ = sink.process_token(Token::NullCharacterToken, 1);
        }
        assert!(matches!(&sink.events.borrow()[..], [Event::Text(false)]));
    }

    #[test]
    fn svg_in_a_caption_is_no_pointer_and_spares_the_next() {
        let html = format!(
            "<figure data-backbone-code=\"1\" data-repo=\"owner/name\" data-commit=\"{A}\" data-path=\"a.txt\">\n  <figcaption><svg><![CDATA[</figcaption> ]]></figcaption>\n</figure>\n<figure data-backbone-code=\"1\" data-repo=\"owner/name\" data-commit=\"{A}\" data-path=\"b.txt\"></figure>"
        );
        assert!(matches!(&pointers(&html)[..], [Pointer::Code { path, .. }] if path == "b.txt"));
        let math = diff("", "<figcaption><math><mi>x</mi></math></figcaption>");
        assert_eq!(pointers(&math), vec![]);
    }

    #[test]
    fn at_most_the_limit_is_read() {
        let html = code("").repeat(LIMIT + 3);
        assert_eq!(find(&html, &later()).len(), LIMIT);
    }
}
