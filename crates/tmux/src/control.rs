//! Decoding the output of a tmux control-mode client (`tmux -C`).
//!
//! tmux writes one message per line. Outside a command's reply, every line is
//! a notification that starts with `%`, such as `%output`. A reply is the
//! lines between `%begin` and the matching `%end` or `%error`. In `%output`,
//! tmux writes every byte below a space, and the backslash, as a backslash and
//! three octal digits; [`Decoder`] removes that escaping.

/// A pane's identifier: the number tmux shows after `%`, as in `%3`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PaneId(pub u32);

/// One message from a control-mode client.
#[derive(Debug, PartialEq, Eq)]
pub enum Message<'a> {
    /// Output from a pane, with tmux's escaping removed. It arrives as
    /// `%output`, or as `%extended-output` when the client has set
    /// `pause-after`.
    Output {
        /// The pane that wrote the output.
        pane: PaneId,
        /// The bytes the pane wrote.
        data: &'a [u8],
    },
    /// The start of the reply to command `number`.
    Begin {
        /// The number tmux gave the command.
        number: u64,
    },
    /// A line of a reply, without its line end.
    ReplyLine(&'a [u8]),
    /// The end of a successful reply to command `number`.
    End {
        /// The number tmux gave the command.
        number: u64,
    },
    /// The end of the reply to command `number`, which failed.
    Error {
        /// The number tmux gave the command.
        number: u64,
    },
    /// Any other notification, such as `%window-add @1`, as the whole line
    /// without its line end.
    Notification(&'a [u8]),
}

/// An error in what a control-mode client wrote.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    /// A line that does not follow the control-mode protocol.
    #[error("malformed control-mode line: {0}")]
    Malformed(&'static str),
}

/// Splits a control-mode client's output into [`Message`]s.
///
/// Output can arrive in pieces of any size: a line split between two calls to
/// [`Decoder::decode`] is kept until the rest of it arrives.
#[derive(Debug, Default)]
pub struct Decoder {
    /// The start of a line whose end has not arrived yet.
    partial: Vec<u8>,
    /// The unescaped data of the current `%output` line.
    output: Vec<u8>,
    /// The reply being read: its command number and the rest of its
    /// `%begin` line, which the closing guard line repeats.
    reply: Option<(u64, Vec<u8>)>,
}

impl Decoder {
    /// Creates a decoder for a new connection.
    pub fn new() -> Self {
        Self::default()
    }

    /// Decodes the next piece of output, calling `on_message` for every line
    /// that is now complete, in order.
    pub fn decode(
        &mut self,
        mut bytes: &[u8],
        mut on_message: impl FnMut(Message<'_>),
    ) -> Result<(), Error> {
        if !self.partial.is_empty() {
            let Some(end) = bytes.iter().position(|&byte| byte == b'\n') else {
                self.partial.extend_from_slice(bytes);
                return Ok(());
            };
            self.partial.extend_from_slice(&bytes[..end]);
            bytes = &bytes[end + 1..];
            let line = std::mem::take(&mut self.partial);
            let decoded = self.line(&line, &mut on_message);
            // Keep the allocation for the next partial line.
            self.partial = line;
            self.partial.clear();
            decoded?;
        }
        while let Some(end) = bytes.iter().position(|&byte| byte == b'\n') {
            self.line(&bytes[..end], &mut on_message)?;
            bytes = &bytes[end + 1..];
        }
        self.partial.extend_from_slice(bytes);
        Ok(())
    }

    fn line(&mut self, line: &[u8], on_message: &mut impl FnMut(Message<'_>)) -> Result<(), Error> {
        if let Some((number, guard)) = &self.reply {
            // A reply ends only at a guard line with the same time, number
            // and flags as its `%begin`: the reply's own lines can start with
            // anything, `%end` included.
            let number = *number;
            let end = match line.strip_prefix(b"%end ") {
                Some(rest) => Some((rest, Message::End { number })),
                None => line
                    .strip_prefix(b"%error ")
                    .map(|rest| (rest, Message::Error { number })),
            };
            match end {
                Some((rest, end)) if rest == guard.as_slice() => {
                    self.reply = None;
                    on_message(end);
                }
                _ => on_message(Message::ReplyLine(line)),
            }
            return Ok(());
        }
        let output = match line.strip_prefix(b"%extended-output ") {
            // With `pause-after` set, tmux sends output as
            // `%extended-output %PANE AGE ... : DATA`.
            Some(rest) => {
                let (pane, rest) = split_word(rest);
                let data = rest
                    .windows(3)
                    .position(|separator| separator == b" : ")
                    .map(|start| &rest[start + 3..])
                    .ok_or(Error::Malformed("%extended-output without its data"))?;
                Some((pane, data))
            }
            None => line.strip_prefix(b"%output ").map(split_word),
        };
        if let Some((pane, data)) = output {
            let pane = pane
                .strip_prefix(b"%")
                .and_then(parse_number)
                .and_then(|number| u32::try_from(number).ok())
                .ok_or(Error::Malformed("%output without a pane"))?;
            unescape(data, &mut self.output)?;
            on_message(Message::Output {
                pane: PaneId(pane),
                data: &self.output,
            });
        } else if let Some(rest) = line.strip_prefix(b"%begin ") {
            let number = guard_number(rest).ok_or(Error::Malformed(
                "%begin without a time, a number and flags",
            ))?;
            self.reply = Some((number, rest.to_vec()));
            on_message(Message::Begin { number });
        } else if line.starts_with(b"%") {
            on_message(Message::Notification(line));
        } else {
            return Err(Error::Malformed(
                "a line outside a reply that is not a notification",
            ));
        }
        Ok(())
    }
}

/// Returns the command number from the rest of a guard line, which is three
/// numbers: the time, the command number and the flags.
fn guard_number(rest: &[u8]) -> Option<u64> {
    let mut fields = rest.split(|&byte| byte == b' ');
    let (Some(time), Some(number), Some(flags), None) =
        (fields.next(), fields.next(), fields.next(), fields.next())
    else {
        return None;
    };
    parse_number(time)?;
    parse_number(flags)?;
    parse_number(number)
}

/// Splits off the first space-separated word.
fn split_word(bytes: &[u8]) -> (&[u8], &[u8]) {
    match bytes.iter().position(|&byte| byte == b' ') {
        Some(space) => (&bytes[..space], &bytes[space + 1..]),
        None => (bytes, &[]),
    }
}

fn parse_number(digits: &[u8]) -> Option<u64> {
    if digits.is_empty() || !digits.iter().all(u8::is_ascii_digit) {
        return None;
    }
    std::str::from_utf8(digits).ok()?.parse().ok()
}

/// Replaces `out` with `escaped` with tmux's `\ooo` escapes decoded.
fn unescape(escaped: &[u8], out: &mut Vec<u8>) -> Result<(), Error> {
    out.clear();
    let mut rest = escaped;
    while let Some(backslash) = rest.iter().position(|&byte| byte == b'\\') {
        out.extend_from_slice(&rest[..backslash]);
        let value = match rest.get(backslash + 1..backslash + 4) {
            Some(&[a, b, c]) if [a, b, c].iter().all(|digit| (b'0'..=b'7').contains(digit)) => {
                u32::from(a - b'0') << 6 | u32::from(b - b'0') << 3 | u32::from(c - b'0')
            }
            _ => return Err(Error::Malformed("a backslash without three octal digits")),
        };
        out.push(u8::try_from(value).map_err(|_| Error::Malformed("an escape above 255"))?);
        rest = &rest[backslash + 4..];
    }
    out.extend_from_slice(rest);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A message with its borrowed bytes copied, so tests can collect them.
    #[derive(Debug, PartialEq, Eq)]
    enum Owned {
        Output(u32, Vec<u8>),
        Begin(u64),
        ReplyLine(Vec<u8>),
        End(u64),
        Error(u64),
        Notification(Vec<u8>),
    }

    fn owned(message: Message<'_>) -> Owned {
        match message {
            Message::Output { pane, data } => Owned::Output(pane.0, data.to_vec()),
            Message::Begin { number } => Owned::Begin(number),
            Message::ReplyLine(line) => Owned::ReplyLine(line.to_vec()),
            Message::End { number } => Owned::End(number),
            Message::Error { number } => Owned::Error(number),
            Message::Notification(line) => Owned::Notification(line.to_vec()),
        }
    }

    fn decode_in_pieces(input: &[u8], piece: usize) -> Result<Vec<Owned>, Error> {
        let mut decoder = Decoder::new();
        let mut messages = Vec::new();
        for bytes in input.chunks(piece) {
            decoder.decode(bytes, |message| messages.push(owned(message)))?;
        }
        Ok(messages)
    }

    const SESSION: &[u8] = b"%begin 1700000000 12 0\n\
        %end 1700000000 12 0\n\
        %session-changed $0 fixture\n\
        %output %3 a\\134b\\015\\012\xe7\x95\x8c\\033[0m\n\
        %extended-output %4 120 : x\\040: y\n\
        %extended-output %4 0 later fields : z\n\
        %begin 1700000001 13 1\n\
        %output %3 not output\n\
        %end 1700000001 99 1\n\
        %end nonsense 13 trailing text\n\
        %end 1700000001 13 1 extra\n\
        %end 1700000001 13 1\n\
        %begin 1700000002 14 1\n\
        parse error\n\
        %error 1700000002 14 1\n\
        %exit\n";

    fn expected() -> Vec<Owned> {
        vec![
            Owned::Begin(12),
            Owned::End(12),
            Owned::Notification(b"%session-changed $0 fixture".to_vec()),
            Owned::Output(3, b"a\\b\r\n\xe7\x95\x8c\x1b[0m".to_vec()),
            Owned::Output(4, b"x : y".to_vec()),
            Owned::Output(4, b"z".to_vec()),
            Owned::Begin(13),
            Owned::ReplyLine(b"%output %3 not output".to_vec()),
            Owned::ReplyLine(b"%end 1700000001 99 1".to_vec()),
            Owned::ReplyLine(b"%end nonsense 13 trailing text".to_vec()),
            Owned::ReplyLine(b"%end 1700000001 13 1 extra".to_vec()),
            Owned::End(13),
            Owned::Begin(14),
            Owned::ReplyLine(b"parse error".to_vec()),
            Owned::Error(14),
            Owned::Notification(b"%exit".to_vec()),
        ]
    }

    #[test]
    fn decodes_output_replies_and_notifications() {
        assert_eq!(
            decode_in_pieces(SESSION, SESSION.len()).unwrap(),
            expected()
        );
    }

    #[test]
    fn decodes_the_same_messages_from_pieces_of_any_size() {
        for piece in 1..SESSION.len() {
            assert_eq!(
                decode_in_pieces(SESSION, piece).unwrap(),
                expected(),
                "piece {piece}"
            );
        }
    }

    #[test]
    fn rejects_malformed_lines() {
        for line in [
            &b"%output %3 bad\\01\n"[..],
            b"%output %3 bad\\9999\n",
            b"%output %3 bad\\777\n",
            b"%output 3 no percent\n",
            b"%extended-output %3 120 no separator\n",
            b"%begin 1700000000 x 0\n",
            b"%begin 1700000000 12\n",
            b"stray text\n",
        ] {
            assert!(decode_in_pieces(line, line.len()).is_err(), "{line:?}");
        }
    }
}
