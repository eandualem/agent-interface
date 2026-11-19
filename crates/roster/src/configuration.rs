//! The API key line of a profile's credential file.

fn whitespace(ch: char) -> bool {
    ch.is_whitespace() || matches!(ch, '\u{1c}'..='\u{1f}')
}

pub(crate) fn dotenv_key(contents: &str) -> Option<String> {
    let mut key = String::new();
    // Python's splitlines also recognizes these single-character boundaries.
    for line in contents.split([
        '\n', '\r', '\u{b}', '\u{c}', '\u{1c}', '\u{1d}', '\u{1e}', '\u{85}', '\u{2028}',
        '\u{2029}',
    ]) {
        let mut line = line.trim_start_matches(whitespace);
        if let Some(rest) = line.strip_prefix("export")
            && rest.starts_with(whitespace)
        {
            line = rest.trim_start_matches(whitespace);
        }
        let Some(rest) = line.strip_prefix("BACKBONE_API_KEY") else {
            continue;
        };
        let Some(value) = rest.trim_start_matches(whitespace).strip_prefix('=') else {
            continue;
        };
        key = first_word(value.trim_start_matches(whitespace))?;
    }
    Some(key)
}

// Match shlex.split(..., comments=True): preserve backslashes before dollar
// signs/backticks inside double quotes, and start comments even within a word.
// Only the first word is retained, but malformed trailing words still fail.
fn first_word(value: &str) -> Option<String> {
    let mut chars = value.chars();
    let mut quote = None;
    let mut in_word = false;
    let mut finished = false;
    let mut first = String::new();
    while let Some(ch) = chars.next() {
        if quote == Some('\'') {
            if ch == '\'' {
                quote = None;
            } else if !finished {
                first.push(ch);
            }
        } else if ch == '\\' {
            let escaped = chars.next()?;
            if !finished {
                if quote == Some('"') && !matches!(escaped, '"' | '\\') {
                    first.push('\\');
                }
                first.push(escaped);
            }
            in_word = true;
        } else if quote == Some('"') {
            if ch == '"' {
                quote = None;
            } else if !finished {
                first.push(ch);
            }
        } else {
            match ch {
                '\'' | '"' => {
                    quote = Some(ch);
                    in_word = true;
                }
                '#' => break,
                ' ' | '\t' | '\r' | '\n' => {
                    if in_word {
                        finished = true;
                        in_word = false;
                    }
                }
                _ => {
                    if !finished {
                        first.push(ch);
                    }
                    in_word = true;
                }
            }
        }
    }
    quote.is_none().then_some(first)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_named_key_is_read_with_python_quotes_and_comments() {
        let text = "OTHER='unterminated\nBACKBONE_API_KEY=first\n export BACKBONE_API_KEY = 'a b'\"c\"#comment\nBACKBONE_API_KEY_EXTRA=ignored\n";
        assert_eq!(dotenv_key(text).as_deref(), Some("a bc"));
        assert_eq!(
            dotenv_key("BACKBONE_API_KEY=one\\ two extra").as_deref(),
            Some("one two")
        );
        assert_eq!(
            dotenv_key("BACKBONE_API_KEY=\"a\\$b\\`c\"").as_deref(),
            Some("a\\$b\\`c")
        );
        assert_eq!(
            dotenv_key("BACKBONE_API_KEY=before#comment").as_deref(),
            Some("before")
        );
        assert_eq!(
            dotenv_key("BACKBONE_API_KEY=before\rBACKBONE_API_KEY=\"\" later").as_deref(),
            Some("")
        );
        assert_eq!(
            dotenv_key("BACKBONE_API_KEY=x\u{2028}BACKBONE_API_KEY= # cleared").as_deref(),
            Some("")
        );
        assert!(dotenv_key("BACKBONE_API_KEY=first 'unterminated").is_none());
        assert!(dotenv_key("BACKBONE_API_KEY=first second\\").is_none());
    }
}
