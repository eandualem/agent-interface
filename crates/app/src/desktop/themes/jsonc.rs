//! JSON with comments and trailing commas, as VS Code writes theme and
//! settings files: a lexer for colouring the editor, a reader that keeps
//! every error's line and column, and a way to find a key's line.

use std::ops::Range;

/// What a piece of the text is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A string that names an object member.
    Key,
    String,
    Number,
    /// `true`, `false` or `null`.
    Literal,
    /// `{ } [ ] : ,`
    Punct,
    Comment,
    /// Anything JSON does not allow, such as an unterminated string.
    Invalid,
}

/// A piece of the text by its byte range.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Token {
    pub kind: Kind,
    pub range: Range<usize>,
}

/// Split `text` into tokens; whitespace is left out.
pub fn lex(text: &str) -> Vec<Token> {
    let bytes = text.as_bytes();
    let mut tokens = Vec::new();
    let mut at = 0;
    while at < bytes.len() {
        let start = at;
        let kind = match bytes[at] {
            b' ' | b'\t' | b'\r' | b'\n' => {
                at += 1;
                continue;
            }
            b'{' | b'}' | b'[' | b']' | b':' | b',' => {
                at += 1;
                Kind::Punct
            }
            b'/' if bytes.get(at + 1) == Some(&b'/') => {
                at = text[at..].find('\n').map_or(bytes.len(), |end| at + end);
                Kind::Comment
            }
            b'/' if bytes.get(at + 1) == Some(&b'*') => match text[at + 2..].find("*/") {
                Some(end) => {
                    at += 2 + end + 2;
                    Kind::Comment
                }
                None => {
                    at = bytes.len();
                    Kind::Invalid
                }
            },
            b'"' => {
                at += 1;
                let mut closed = false;
                while at < bytes.len() {
                    match bytes[at] {
                        b'\\' => at += 2,
                        b'"' => {
                            at += 1;
                            closed = true;
                            break;
                        }
                        b'\n' => break,
                        _ => at += 1,
                    }
                }
                at = at.min(bytes.len());
                if closed { Kind::String } else { Kind::Invalid }
            }
            b'-' | b'0'..=b'9' => {
                at += 1;
                while at < bytes.len()
                    && matches!(bytes[at], b'0'..=b'9' | b'.' | b'e' | b'E' | b'+' | b'-')
                {
                    at += 1;
                }
                Kind::Number
            }
            _ => {
                // A word, or one character JSON does not know.
                let word = text[at..]
                    .char_indices()
                    .find(|(_, ch)| !ch.is_alphanumeric() && *ch != '_')
                    .map_or(text.len() - at, |(end, _)| end);
                let length = word.max(text[at..].chars().next().map_or(1, char::len_utf8));
                at += length;
                if matches!(&text[start..at], "true" | "false" | "null") {
                    Kind::Literal
                } else {
                    Kind::Invalid
                }
            }
        };
        tokens.push(Token {
            kind,
            range: start..at,
        });
    }
    // A string followed by a colon names a member.
    for at in 0..tokens.len() {
        if tokens[at].kind == Kind::String
            && next_significant(&tokens, at).is_some_and(|next| &text[next.range.clone()] == ":")
        {
            tokens[at].kind = Kind::Key;
        }
    }
    tokens
}

fn next_significant(tokens: &[Token], at: usize) -> Option<&Token> {
    tokens[at + 1..]
        .iter()
        .find(|token| token.kind != Kind::Comment)
}

/// `text` with comments and trailing commas blanked out, every other byte
/// and every line break kept, so positions stay those of the original.
pub fn strip(text: &str) -> String {
    let tokens = lex(text);
    let mut bytes = text.as_bytes().to_vec();
    let mut blank = |range: Range<usize>| {
        for byte in &mut bytes[range] {
            if *byte != b'\n' {
                *byte = b' ';
            }
        }
    };
    for (at, token) in tokens.iter().enumerate() {
        let trailing_comma = &text[token.range.clone()] == ","
            && next_significant(&tokens, at)
                .is_some_and(|next| matches!(&text[next.range.clone()], "}" | "]"));
        if token.kind == Kind::Comment || trailing_comma {
            blank(token.range.clone());
        }
    }
    // Only whole multi-byte characters were blanked, so this stays UTF-8.
    String::from_utf8(bytes).unwrap_or_else(|_| text.to_owned())
}

/// A reading error at a one-based line and column, when it has a place.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error {
    pub message: String,
    pub line: Option<usize>,
    pub column: Option<usize>,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match (self.line, self.column) {
            (Some(line), Some(column)) => {
                write!(f, "Line {line}, column {column}: {}", self.message)
            }
            (Some(line), None) => write!(f, "Line {line}: {}", self.message),
            _ => f.write_str(&self.message),
        }
    }
}

impl Error {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            line: None,
            column: None,
        }
    }
}

/// Read JSON that may hold comments and trailing commas.
pub fn parse(text: &str) -> Result<serde_json::Value, Error> {
    serde_json::from_str(&strip(text)).map_err(|error| {
        let message = error.to_string();
        // serde_json ends its message with its own " at line L column C".
        let message = message
            .rsplit_once(" at line ")
            .map_or(message.as_str(), |(message, _)| message)
            .to_owned();
        Error {
            message,
            line: (error.line() > 0).then_some(error.line()),
            column: (error.column() > 0).then_some(error.column()),
        }
    })
}

/// The one-based line and column of the member at `path` (keys, or indices
/// for arrays), or of its nearest named parent.
pub fn locate(text: &str, path: &[&str]) -> Option<(usize, usize)> {
    struct Frame {
        array: bool,
        key: Option<String>,
        index: usize,
    }
    let tokens = lex(text);
    let mut stack: Vec<Frame> = Vec::new();
    let mut best: Option<(usize, usize)> = None;
    let mut best_depth = 0;
    let here = |stack: &[Frame]| -> Vec<String> {
        stack
            .iter()
            .filter_map(|frame| {
                if frame.array {
                    Some(frame.index.to_string())
                } else {
                    frame.key.clone()
                }
            })
            .collect()
    };
    for token in &tokens {
        let piece = &text[token.range.clone()];
        match (token.kind, piece) {
            (Kind::Key, _) => {
                let key = serde_json::from_str::<String>(piece).unwrap_or_default();
                if let Some(frame) = stack.last_mut() {
                    frame.key = Some(key);
                }
                let at = here(&stack);
                let matched = at.len() <= path.len() && at.iter().zip(path).all(|(a, b)| a == b);
                if matched && at.len() > best_depth {
                    best_depth = at.len();
                    best = Some(position(text, token.range.start));
                    if at.len() == path.len() {
                        return best;
                    }
                }
            }
            (Kind::Punct, "{" | "[") => {
                if let Some(frame) = stack.last()
                    && frame.array
                {
                    let at = here(&stack);
                    if at.len() == path.len() && at.iter().zip(path).all(|(a, b)| a == b) {
                        return Some(position(text, token.range.start));
                    }
                }
                stack.push(Frame {
                    array: piece == "[",
                    key: None,
                    index: 0,
                })
            }
            (Kind::Punct, "}" | "]") => {
                stack.pop();
            }
            (Kind::Punct, ",") => {
                if let Some(frame) = stack.last_mut()
                    && frame.array
                {
                    frame.index += 1;
                }
            }
            _ => {}
        }
    }
    best
}

/// The one-based line and column of a byte offset, the column in bytes as
/// the reader counts it.
pub fn position(text: &str, offset: usize) -> (usize, usize) {
    let before = &text[..offset.min(text.len())];
    let line = before.matches('\n').count() + 1;
    let column = before.rsplit('\n').next().map_or(0, str::len) + 1;
    (line, column)
}

/// The byte offset of a one-based line and column, the column in bytes as
/// the reader counts it.
pub fn offset(text: &str, line: usize, column: usize) -> usize {
    let start: usize = text
        .split_inclusive('\n')
        .take(line.saturating_sub(1))
        .map(str::len)
        .sum();
    let line_end = text[start.min(text.len())..]
        .find('\n')
        .map_or(text.len(), |end| start + end);
    let mut at = (start + column.saturating_sub(1)).min(line_end);
    while !text.is_char_boundary(at) {
        at -= 1;
    }
    at
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comments_and_trailing_commas_are_read_and_positions_are_kept() {
        let text = "{\n  // a comment é\n  \"a\": [1, 2,], /* b */\n  \"b\": true,\n}\n";
        let stripped = strip(text);
        assert_eq!(stripped.len(), text.len());
        assert_eq!(stripped.lines().count(), text.lines().count());
        let value = parse(text).unwrap();
        assert_eq!(value["a"][1], 2);
        assert_eq!(value["b"], true);

        let error = parse("{\n  \"a\": 1\n  \"b\": 2\n}").unwrap_err();
        assert_eq!(error.line, Some(3));
        assert!(error.message.contains("expected"), "{}", error.message);
        assert!(!error.message.contains(" at line "));
    }

    #[test]
    fn the_lexer_names_keys_strings_numbers_and_mistakes() {
        let text = "{\"k\" : \"v\", \"n\": -1.5e3, \"t\": true, \"x\": nope, \"u\": \"open\n}";
        let kinds: Vec<_> = lex(text)
            .into_iter()
            .map(|token| (token.kind, text[token.range].to_owned()))
            .collect();
        assert!(kinds.contains(&(Kind::Key, "\"k\"".into())));
        assert!(kinds.contains(&(Kind::String, "\"v\"".into())));
        assert!(kinds.contains(&(Kind::Number, "-1.5e3".into())));
        assert!(kinds.contains(&(Kind::Literal, "true".into())));
        assert!(kinds.contains(&(Kind::Invalid, "nope".into())));
        assert!(kinds.contains(&(Kind::Invalid, "\"open".into())));
    }

    #[test]
    fn a_member_is_found_by_its_path() {
        let text = "{\n  \"color\": {\n    \"ground\": \"#000\"\n  },\n  \"terminal\": {\n    \"palette\": [\n      \"#1\",\n      \"#2\"\n    ]\n  }\n}";
        assert_eq!(locate(text, &["color", "ground"]), Some((3, 5)));
        assert_eq!(locate(text, &["terminal", "palette"]), Some((6, 5)));
        // A missing member points at its nearest parent.
        assert_eq!(locate(text, &["color", "panel"]), Some((2, 3)));
        assert_eq!(locate(text, &["nothing"]), None);
    }

    #[test]
    fn positions_and_offsets_agree() {
        let text = "ab\ncdé\nf";
        assert_eq!(position(text, 0), (1, 1));
        assert_eq!(position(text, 5), (2, 3));
        assert_eq!(position(text, 7), (2, 5));
        assert_eq!(offset(text, 2, 3), 5);
        assert_eq!(offset(text, 2, 99), 7);
        assert_eq!(offset(text, 3, 1), 8);
    }
}
