//! Static HTML views of diffs and code. Each opens and closes natively as
//! `<details open>`; the host stylesheet (`assets/document/theme.css`) draws
//! them in the theme's `--ai-*` colours. Every text from Git is escaped.

use std::fmt::Write;

use super::git::{File, ROW_LIMIT, Row};

/// A diff's view, or `None` if it would be longer than `limit` bytes. One
/// file is one `<details open>`, its summary the caption, given as authored
/// HTML, with the file's header under it, or else the header. Several files
/// are one `<details open>` per file inside another, whose summary is the
/// caption or their count.
pub fn diff(files: &[File], caption: Option<&str>, limit: usize) -> Option<String> {
    let header = |file: &File| {
        let mut header = String::new();
        if let Some(from) = &file.from {
            escape(from, &mut header);
            header.push_str(" → ");
        }
        escape(&file.path, &mut header);
        // A file with no lines to show says what changed.
        header.push_str(match (file.status, &file.rows[..]) {
            ('A', [] | [Row::Binary]) => " (added)",
            ('D', [] | [Row::Binary]) => " (deleted)",
            ('R', [] | [Row::Binary]) => " (renamed)",
            ('M', []) => " (mode changed)",
            _ => "",
        });
        header
    };
    let mut out = String::new();
    if let [file] = files {
        open(&mut out, caption, &header(file));
        table(&mut out, &file.rows, limit)?;
    } else {
        let count = format!("{} files changed", files.len());
        details(&mut out, "ai-git", caption.unwrap_or(&count));
        // A captioned diff with no files says so under its caption.
        if caption.is_some() && files.is_empty() {
            let _ = write!(out, r#"<div class="ai-git-head">{count}</div>"#);
        }
        for file in files {
            details(&mut out, "ai-git-file", &header(file));
            table(&mut out, &file.rows, limit)?;
            out.push_str("</details>");
        }
    }
    out.push_str("</details>");
    (out.len() <= limit).then_some(out)
}

/// A code view of `text`, numbered from its first line, or `None` when the
/// file has none of `lines`, or more than [`ROW_LIMIT`] of them, or the view
/// would be longer than `limit` bytes. Its summary is the caption, with the
/// file's header under it, or else the header.
pub fn code(
    path: &str,
    text: &str,
    lines: Option<(u32, u32)>,
    caption: Option<&str>,
    limit: usize,
) -> Option<String> {
    let all = text.lines().count();
    let (first, last) = match lines {
        Some((first, last)) => (first as usize, (last as usize).min(all)),
        None => (1, all),
    };
    if (lines.is_some() && first > all) || last + 1 - first > ROW_LIMIT {
        return None;
    }
    let mut header = String::new();
    escape(path, &mut header);
    if lines.is_some() {
        let _ = write!(header, " · lines {first}–{last}");
    }
    let mut out = String::new();
    open(&mut out, caption, &header);
    out.push_str(r#"<div class="ai-git-scroll"><table class="ai-git-lines"><tbody>"#);
    let shown = text
        .lines()
        .enumerate()
        .skip(first - 1)
        .take(last + 1 - first);
    for (number, line) in shown {
        let _ = write!(
            out,
            r#"<tr><td class="ai-git-n">{}</td><td class="ai-git-t">"#,
            number + 1
        );
        escape(line, &mut out);
        out.push_str("</td></tr>");
        if out.len() > limit {
            return None;
        }
    }
    out.push_str("</tbody></table></div></details>");
    (out.len() <= limit).then_some(out)
}

/// Open a view of one file, summarised by the caption or its header.
fn open(out: &mut String, caption: Option<&str>, header: &str) {
    match caption {
        Some(caption) => {
            details(out, "ai-git", caption);
            let _ = write!(out, r#"<div class="ai-git-head">{header}</div>"#);
        }
        None => details(out, "ai-git ai-git-file", header),
    }
}

fn details(out: &mut String, class: &str, summary: &str) {
    let _ = write!(
        out,
        r#"<details open class="{class}"><summary>{summary}</summary>"#
    );
}

/// A file's rows: hunk headers, then lines with their old and new numbers
/// and their sign as text. `None` once `out` is longer than `limit`.
fn table(out: &mut String, rows: &[Row], limit: usize) -> Option<()> {
    if rows.is_empty() {
        return Some(());
    }
    out.push_str(r#"<div class="ai-git-scroll"><table class="ai-git-lines"><tbody>"#);
    for row in rows {
        match row {
            Row::Hunk(text) | Row::Remark(text) => {
                let class = if matches!(row, Row::Hunk(_)) {
                    "ai-git-hunk"
                } else {
                    "ai-git-remark"
                };
                let _ = write!(out, r#"<tr class="{class}"><td colspan="4">"#);
                escape(text, out);
                out.push_str("</td></tr>");
            }
            Row::Binary => {
                out.push_str(r#"<tr class="ai-git-remark"><td colspan="4">Binary file</td></tr>"#)
            }
            Row::Line {
                sign,
                old,
                new,
                text,
            } => {
                let class = match sign {
                    '+' => "ai-git-add",
                    '-' => "ai-git-remove",
                    _ => "ai-git-same",
                };
                let number =
                    |number: Option<u32>| number.map(|n| n.to_string()).unwrap_or_default();
                let _ = write!(
                    out,
                    r#"<tr class="{class}"><td class="ai-git-n">{}</td><td class="ai-git-n">{}</td><td class="ai-git-s">{sign}</td><td class="ai-git-t">"#,
                    number(*old),
                    number(*new),
                );
                escape(text, out);
                out.push_str("</td></tr>");
            }
        }
        if out.len() > limit {
            return None;
        }
    }
    out.push_str("</tbody></table></div>");
    Some(())
}

/// `text` as HTML text or an attribute value.
fn escape(text: &str, out: &mut String) {
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            // A literal CR would reach the page as a line break.
            '\r' => out.push_str("&#13;"),
            _ => out.push(c),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ANY: usize = usize::MAX;

    fn file(path: &str, rows: Vec<Row>) -> File {
        File {
            status: 'M',
            from: None,
            path: path.into(),
            rows,
        }
    }

    #[test]
    fn a_diff_shows_numbers_signs_and_escaped_text() {
        let rows = vec![
            Row::Hunk("@@ -1,2 +1,2 @@ fn <a>".into()),
            Row::Line {
                sign: ' ',
                old: Some(1),
                new: Some(1),
                text: "same".into(),
            },
            Row::Line {
                sign: '-',
                old: Some(2),
                new: None,
                text: "<script>alert('x')</script> & \"q\"".into(),
            },
            Row::Line {
                sign: '+',
                old: None,
                new: Some(2),
                text: "new".into(),
            },
            Row::Remark("\\ No newline at end of file".into()),
        ];
        let html = diff(&[file("src/<a>.rs", rows)], None, ANY).unwrap();
        assert!(html.starts_with(
            r#"<details open class="ai-git ai-git-file"><summary>src/&lt;a&gt;.rs</summary>"#
        ));
        assert!(html.contains(r#"<td colspan="4">@@ -1,2 +1,2 @@ fn &lt;a&gt;</td>"#));
        assert!(html.contains(r#"<tr class="ai-git-same"><td class="ai-git-n">1</td><td class="ai-git-n">1</td><td class="ai-git-s"> </td><td class="ai-git-t">same</td></tr>"#));
        assert!(html.contains(r#"<tr class="ai-git-remove"><td class="ai-git-n">2</td><td class="ai-git-n"></td><td class="ai-git-s">-</td><td class="ai-git-t">&lt;script&gt;alert(&#39;x&#39;)&lt;/script&gt; &amp; &quot;q&quot;</td></tr>"#));
        assert!(html.contains(
            r#"<td class="ai-git-n"></td><td class="ai-git-n">2</td><td class="ai-git-s">+</td>"#
        ));
        assert!(html.contains("ai-git-remark"));
        assert!(!html.contains("<script"));
        // Past its limit, a view is not made.
        assert_eq!(diff(&[file("a.rs", vec![Row::Binary])], None, 100), None);
    }

    #[test]
    fn one_file_is_one_details_and_several_are_one_each_inside() {
        // A binary file still says it was renamed.
        let mut renamed = file("new.rs", vec![Row::Binary]);
        renamed.from = Some("old.rs".into());
        renamed.status = 'R';
        let html = diff(&[file("a.rs", vec![]), renamed.clone()], None, ANY).unwrap();
        assert!(html.starts_with(r#"<details open class="ai-git"><summary>2 files changed</summary><details open class="ai-git-file"><summary>a.rs (mode changed)</summary></details>"#));
        assert!(html.contains("<summary>old.rs → new.rs (renamed)</summary>"));
        assert!(html.contains("Binary file"));
        let html = diff(&[renamed], Some("The <em>move</em>"), ANY).unwrap();
        assert!(html.starts_with(
            r#"<details open class="ai-git"><summary>The <em>move</em></summary><div class="ai-git-head">old.rs → new.rs (renamed)</div><div"#
        ));
        assert_eq!(html.matches("<details").count(), 1);
        assert!(html.ends_with("</details>"));
        let none = diff(&[], Some("cap"), ANY).unwrap();
        assert!(
            none.contains(
                r#"<summary>cap</summary><div class="ai-git-head">0 files changed</div>"#
            )
        );
        assert!(
            diff(&[], None, ANY)
                .unwrap()
                .contains("<summary>0 files changed</summary>")
        );
    }

    #[test]
    fn a_carriage_return_is_kept_as_one() {
        let crlf = Row::Line {
            sign: '+',
            old: None,
            new: Some(1),
            text: "windows\r".into(),
        };
        let html = diff(&[file("a.txt", vec![crlf])], None, ANY).unwrap();
        assert!(html.contains(r#"<td class="ai-git-t">windows&#13;</td>"#));
        let html = code("a.txt", "one\r\nlone\rcr\n", None, None, ANY).unwrap();
        assert!(html.contains(r#"<td class="ai-git-t">one</td>"#));
        assert!(html.contains(r#"<td class="ai-git-t">lone&#13;cr</td>"#));
        assert!(!html.contains('\r'));
    }

    #[test]
    fn code_is_numbered_from_its_first_line() {
        let text = "one\ntwo\nthree\n";
        let html = code("a.rs", text, Some((2, 9)), None, ANY).unwrap();
        assert!(html.contains("<summary>a.rs · lines 2–3</summary>"));
        assert!(html.contains(r#"<td class="ai-git-n">2</td><td class="ai-git-t">two</td>"#));
        assert!(html.contains(r#"<td class="ai-git-n">3</td><td class="ai-git-t">three</td>"#));
        assert!(!html.contains(">one<"));
        let whole = code("a.rs", text, None, Some("cap"), ANY).unwrap();
        assert!(whole.starts_with(
            r#"<details open class="ai-git"><summary>cap</summary><div class="ai-git-head">a.rs</div>"#
        ));
        assert_eq!(whole.matches("<details").count(), 1);
        assert_eq!(whole.matches("<tr>").count(), 3);
        assert_eq!(code("a.rs", text, Some((4, 4)), None, ANY), None);
        assert!(code("a.rs", "", None, None, ANY).is_some());
        assert_eq!(
            code("a.rs", &"x\n".repeat(ROW_LIMIT + 1), None, None, ANY),
            None
        );
        assert_eq!(code("a.rs", text, None, None, 100), None);
    }
}
