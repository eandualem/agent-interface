//! The look of reading elements, wherever they are drawn. Native Markdown
//! draws tables, code, quotes and rules from it, and the stylesheet given to
//! HTML documents is built from it, so the same element looks the same in a
//! skill, an answer and a document.

use gpui::Rgba;

use crate::desktop::design::colors;

/// The colours of each reading element in the theme in use.
pub(super) struct Reading {
    /// A quiet fill behind a table header or inline code.
    pub tint: Rgba,
    pub table_head: Rgba,
    pub table_line: Rgba,
    pub code_text: Rgba,
    pub code_fill: Rgba,
    /// A block of code: its fill and its border.
    pub block_fill: Rgba,
    pub block_line: Rgba,
    pub quote_bar: Rgba,
    pub quote_text: Rgba,
    pub rule: Rgba,
    pub link: Rgba,
    /// Behind selected text, which keeps its own colour.
    pub selection: Rgba,
}

/// `from` moved `amount` of the way to `to`, opaque.
fn mix(from: Rgba, to: Rgba, amount: f32) -> Rgba {
    let part = |from: f32, to: f32| from + (to - from) * amount;
    Rgba {
        r: part(from.r, to.r),
        g: part(from.g, to.g),
        b: part(from.b, to.b),
        a: 1.,
    }
}

pub(super) fn look() -> Reading {
    let c = colors();
    // The page with a little of its text colour: visible in every theme
    // without the weight of a selected row.
    let tint = mix(c.surface, c.text, 0.05);
    Reading {
        tint,
        table_head: tint,
        table_line: c.line,
        code_text: c.code,
        code_fill: tint,
        block_fill: c.ground,
        block_line: c.line,
        quote_bar: c.line_strong,
        quote_text: c.muted,
        rule: c.line,
        link: c.accent_text,
        selection: Rgba {
            a: 0.35,
            ..c.accent
        },
    }
}

/// A diff's added and removed lines: each one's fill, and a colour for its
/// sign that reads on that fill at 4.5:1, darkened or lightened if needed.
fn diff_colours(c: &crate::desktop::design::Colors) -> [(Rgba, Rgba); 2] {
    use crate::desktop::themes::vscode::contrast;
    let readable = |ink: Rgba, fill: Rgba| {
        // Black or white, whichever reads better, reads at 4.5:1 or more on
        // any fill.
        let [black, white] = [0., 1.].map(|v| Rgba {
            r: v,
            g: v,
            b: v,
            a: 1.,
        });
        let far = if contrast(black, fill) > contrast(white, fill) {
            black
        } else {
            white
        };
        (0..=10)
            .map(|step| byte_exact(mix(ink, far, step as f32 / 10.)))
            .find(|ink| contrast(*ink, fill) >= 4.5)
            .unwrap_or(far)
    };
    // Checked as the page gets them: in whole bytes, as `hex` writes them.
    let add = byte_exact(mix(c.surface, c.ok, 0.16));
    let remove = byte_exact(mix(c.surface, c.attention, 0.16));
    [
        (add, readable(c.ok, add)),
        (remove, readable(c.attention_text, remove)),
    ]
}

/// `color` with each channel rounded to the byte `hex` writes.
fn byte_exact(color: Rgba) -> Rgba {
    let part = |value: f32| (value.clamp(0., 1.) * 255.).round() / 255.;
    Rgba {
        r: part(color.r),
        g: part(color.g),
        b: part(color.b),
        a: color.a,
    }
}

/// `color` as CSS.
fn hex(color: Rgba) -> String {
    let part = |value: f32| (value.clamp(0., 1.) * 255.).round() as u8;
    let (r, g, b) = (part(color.r), part(color.g), part(color.b));
    if color.a < 1. {
        format!("rgba({r}, {g}, {b}, {:.3})", color.a.max(0.))
    } else {
        format!("#{r:02x}{g:02x}{b:02x}")
    }
}

/// The script that draws an HTML document in the theme in use: its colours
/// mapped onto the theme, then the reading elements styled as above.
pub(super) fn document_script() -> std::rc::Rc<str> {
    use std::{cell::RefCell, rc::Rc};
    thread_local! {
        /// The script of the last theme asked for; themes live for the process.
        static LAST: RefCell<Option<(usize, Rc<str>)>> = const { RefCell::new(None) };
    }
    let design = crate::desktop::design::tokens();
    let key = std::ptr::from_ref(design) as usize;
    LAST.with_borrow_mut(|last| {
        if let Some((known, script)) = last
            && *known == key
        {
            return script.clone();
        }
        let script: Rc<str> = script(&design.colors, crate::desktop::design::is_light()).into();
        *last = Some((key, script.clone()));
        script
    })
}

fn script(c: &crate::desktop::design::Colors, light: bool) -> String {
    let look = look();
    let [(add, add_sign), (remove, remove_sign)] = diff_colours(c);
    let palette = serde_json::json!({
        "scheme": if light { "light" } else { "dark" },
        // What a document may use as `var(--ai-…)`.
        "variables": {
            "background": hex(c.surface),
            "field": hex(c.field),
            "surface": hex(look.tint),
            "text": hex(c.text),
            "muted": hex(c.muted),
            "accent": hex(look.link),
            "line": hex(look.table_line),
            "line-strong": hex(c.line_strong),
            "attention": hex(c.attention_text),
            "ok": hex(c.ok),
            "info": hex(c.info),
            "violet": hex(c.violet),
            "code": hex(look.code_text),
            "code-fill": hex(look.code_fill),
            "block": hex(look.block_fill),
            "table-head": hex(look.table_head),
            "selection": hex(look.selection),
            // A diff's added and removed lines: their fill and their sign.
            "diff-add": hex(add),
            "diff-add-sign": hex(add_sign),
            "diff-remove": hex(remove),
            "diff-remove-sign": hex(remove_sign),
        },
        // Neutrals, from the page to its text.
        "ramp": {
            "page": hex(c.surface),
            // A card lighter than a light page, or darker than a dark one.
            "raised": hex(if light { c.field } else { c.ground }),
            "tint": hex(look.tint),
            "lineStrong": hex(c.line_strong),
            "faint": hex(c.faint),
            "muted": hex(c.muted),
            "text2": hex(c.text_2),
            "text": hex(c.text),
        },
        // A chromatic colour takes the nearest of these in hue.
        "hues": [
            hex(c.accent_text), hex(c.attention_text), hex(c.ok),
            hex(c.info), hex(c.violet), hex(c.green),
        ],
        "css": include_str!("../../../assets/document/theme.css"),
    });
    include_str!("../../../assets/document/theme.js").replacen(
        "/*PALETTE*/ {}",
        &palette.to_string(),
        1,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn document_script_carries_the_theme_and_the_stylesheet() {
        for light in [false, true] {
            let design = crate::desktop::design::builtin(light);
            let script = script(&design.colors, light);
            let start = script.find("const T = ").expect("palette") + "const T = ".len();
            let end = start + script[start..].find(";\n").expect("palette end");
            let palette: serde_json::Value =
                serde_json::from_str(&script[start..end]).expect("palette is JSON");
            assert_eq!(palette["scheme"], if light { "light" } else { "dark" });
            assert_eq!(palette["variables"]["text"], hex(design.colors.text));
            assert_eq!(palette["hues"].as_array().map(Vec::len), Some(6));
            assert!(
                palette["css"]
                    .as_str()
                    .is_some_and(|css| css.contains("--ai-table-head"))
            );
            assert!(palette["variables"]["diff-add"].is_string());
        }
        assert_eq!(
            hex(Rgba {
                r: 1.,
                g: 0.5,
                b: 0.,
                a: 0.35
            }),
            "rgba(255, 128, 0, 0.350)"
        );
    }

    #[test]
    fn every_rule_of_the_view_ranks_above_a_documents() {
        let css = include_str!("../../../assets/document/theme.css");
        let start = css.find("/* Git diffs and code").expect("the view's rules");
        // The view's rules, without their comments.
        let mut view = String::new();
        let mut rest = &css[start..];
        while let Some(open) = rest.find("/*") {
            view.push_str(&rest[..open]);
            let close = rest[open..].find("*/").expect("comment end");
            rest = &rest[open + close + 2..];
        }
        view.push_str(rest);
        assert!(
            view.contains("all: initial !important") && view.contains("all: revert !important")
        );
        for rule in view
            .split('}')
            .map(str::trim)
            .filter(|rule| !rule.is_empty())
        {
            let (selectors, body) = rule.split_once('{').expect("a rule");
            // Selectors, split at commas outside parentheses.
            let (mut depth, mut from, mut list) = (0, 0, Vec::new());
            for (at, c) in selectors.char_indices() {
                match c {
                    '(' => depth += 1,
                    ')' => depth -= 1,
                    ',' if depth == 0 => {
                        list.push(&selectors[from..at]);
                        from = at + 1;
                    }
                    _ => {}
                }
            }
            list.push(&selectors[from..]);
            for selector in list {
                // The prefix is the view's classes, and only those.
                let rest = selector.trim().strip_prefix(".ai-git.ai-git");
                let after = rest.map(|rest| rest.chars().next());
                assert!(
                    after.is_some_and(
                        |c| c.is_none_or(|c| c.is_whitespace() || ":[>+~.".contains(c))
                    ),
                    "{selector}"
                );
                // A reset of a closed details' content would show it.
                if selector.contains("::details-content") {
                    assert!(selector.contains("[open]::details-content"), "{selector}");
                }
            }
            for declaration in body.split(';').map(str::trim).filter(|d| !d.is_empty()) {
                assert!(declaration.ends_with("!important"), "{declaration}");
            }
        }
    }

    #[test]
    fn diff_signs_read_on_their_fill_in_every_shipped_theme() {
        for id in [
            "dark",
            "light",
            "dracula",
            "one-dark",
            "tokyo-night",
            "catppuccin-mocha",
            "nord",
            "gruvbox-dark",
            "solarized-dark",
            "github-dark",
            "catppuccin-latte",
            "solarized-light",
            "github-light",
            "one-light",
        ] {
            let design = crate::desktop::themes::resolve(id).unwrap();
            // The colours as the page gets them, read back from their CSS.
            let emitted = |color: Rgba| {
                let hex = hex(color);
                let byte = |at: usize| u8::from_str_radix(&hex[at..at + 2], 16).unwrap();
                let [r, g, b] = [1, 3, 5].map(|at| f32::from(byte(at)) / 255.);
                Rgba { r, g, b, a: 1. }
            };
            for (fill, sign) in diff_colours(&design.colors) {
                let ratio = crate::desktop::themes::vscode::contrast(emitted(sign), emitted(fill));
                assert!(ratio >= 4.5, "{id}: {ratio}");
            }
        }
    }
}
