//! Pane rectangles from control-mode layout notifications.

use crate::control::PaneId;

/// One visible pane's rectangle, in terminal cells.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pane {
    /// The pane's server-wide identity.
    pub id: PaneId,
    /// The column of its left edge.
    pub left: u16,
    /// The row of its top edge.
    pub top: u16,
    /// The number of columns.
    pub cols: u16,
    /// The number of rows.
    pub rows: u16,
}

/// The visible panes of a window after its layout changes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    /// The window's server-wide identity, without its `@` prefix.
    pub window: u32,
    /// Visible pane rectangles. A zoomed window contains only its zoomed pane.
    pub panes: Vec<Pane>,
}

/// Reads a complete `%layout-change` notification.
///
/// tmux sends both the saved layout and the visible layout; only the latter
/// describes the rectangles to draw when a pane is zoomed. The four-digit
/// checksum field is checked for valid syntax but is not recomputed. Also
/// accepts tmux 3.8 JSON layouts. Floating panes are rejected explicitly.
pub fn parse(line: &[u8]) -> Result<Change, &'static str> {
    let line = std::str::from_utf8(line).map_err(|_| "layout is not UTF-8")?;
    let mut fields = line.split_ascii_whitespace();
    if fields.next() != Some("%layout-change") {
        return Err("not a layout-change notification");
    }
    let window = fields
        .next()
        .and_then(|id| id.strip_prefix('@'))
        .and_then(|id| id.parse().ok())
        .ok_or("invalid window identity")?;
    fields.next().ok_or("missing saved layout")?;
    let visible = fields.next().ok_or("missing visible layout")?;
    if visible.starts_with('{') {
        return Ok(Change {
            window,
            panes: json_panes(visible)?,
        });
    }
    let (checksum, cells) = visible.split_once(',').ok_or("missing layout checksum")?;
    if checksum.len() != 4 || !checksum.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("invalid layout checksum");
    }

    let mut input = Input(cells.as_bytes());
    let mut closing = Vec::new();
    let mut panes = Vec::new();
    loop {
        let cols = input.cell_number()?;
        input.take(b'x')?;
        let rows = input.cell_number()?;
        input.take(b',')?;
        let left = input.cell_number()?;
        input.take(b',')?;
        let top = input.cell_number()?;
        if cols == 0 || rows == 0 {
            return Err("empty pane rectangle");
        }
        match input.0.first() {
            Some(b'[' | b'{') => {
                closing.push(if input.0[0] == b'[' { b']' } else { b'}' });
                input.0 = &input.0[1..];
                continue;
            }
            Some(b',') => {
                input.0 = &input.0[1..];
                panes.push(Pane {
                    id: PaneId(input.number()?),
                    left,
                    top,
                    cols,
                    rows,
                });
            }
            _ => return Err("missing pane identity or split"),
        }
        // Walk back out of completed splits without recursive parsing: the
        // layout's nesting depth does not consume the call stack.
        loop {
            match (input.0.first(), closing.last()) {
                (None, None) => return Ok(Change { window, panes }),
                (Some(b','), Some(_)) => {
                    input.0 = &input.0[1..];
                    break;
                }
                (Some(byte), Some(expected)) if byte == expected => {
                    input.0 = &input.0[1..];
                    closing.pop();
                }
                _ => return Err("invalid split boundary"),
            }
        }
    }
}

fn json_panes(text: &str) -> Result<Vec<Pane>, &'static str> {
    let value: serde_json::Value = serde_json::from_str(text).map_err(|_| "invalid JSON layout")?;
    if value.get("V").and_then(serde_json::Value::as_u64) != Some(2) {
        return Err("unsupported layout version");
    }
    let mut pending = vec![value.get("L").ok_or("missing layout root")?];
    let mut panes = Vec::new();
    while let Some(cell) = pending.pop() {
        if cell.get("z").is_some() {
            return Err("floating panes are not supported by this view");
        }
        let number = |key| -> Result<u16, &'static str> {
            cell.get(key)
                .and_then(serde_json::Value::as_u64)
                .and_then(|n| n.try_into().ok())
                .ok_or("invalid rectangle number")
        };
        let (cols, rows, left, top) = (number("w")?, number("h")?, number("x")?, number("y")?);
        if cols == 0 || rows == 0 {
            return Err("empty pane rectangle");
        }
        match cell.get("t").and_then(serde_json::Value::as_str) {
            Some("h" | "v") => {
                let children = cell
                    .get("c")
                    .and_then(serde_json::Value::as_array)
                    .ok_or("missing split children")?;
                if children.is_empty() {
                    return Err("empty split");
                }
                pending.extend(children.iter().rev());
            }
            Some("p") => {
                let id = cell
                    .get("I")
                    .and_then(serde_json::Value::as_str)
                    .and_then(|id| id.strip_prefix('%'))
                    .and_then(|id| id.parse().ok())
                    .ok_or("invalid pane identity")?;
                panes.push(Pane {
                    id: PaneId(id),
                    left,
                    top,
                    cols,
                    rows,
                });
            }
            _ => return Err("invalid layout cell type"),
        }
    }
    Ok(panes)
}

struct Input<'a>(&'a [u8]);

impl Input<'_> {
    fn take(&mut self, byte: u8) -> Result<(), &'static str> {
        if self.0.first() != Some(&byte) {
            return Err("invalid rectangle separator");
        }
        self.0 = &self.0[1..];
        Ok(())
    }

    fn number(&mut self) -> Result<u32, &'static str> {
        let count = self
            .0
            .iter()
            .take_while(|byte| byte.is_ascii_digit())
            .count();
        let number = std::str::from_utf8(&self.0[..count])
            .map_err(|_| "invalid layout number")?
            .parse()
            .map_err(|_| "invalid layout number")?;
        self.0 = &self.0[count..];
        Ok(number)
    }

    fn cell_number(&mut self) -> Result<u16, &'static str> {
        self.number()?
            .try_into()
            .map_err(|_| "rectangle is too large")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_nested_splits_and_server_wide_pane_ids() {
        let layout = b"%layout-change @12 abcd,80x24,0,0 abcd,80x24,0,0{39x24,0,0,70000,40x24,40,0[40x11,40,0,2,40x12,40,12,3]} *";
        let change = parse(layout).unwrap();
        assert_eq!(change.window, 12);
        assert_eq!(
            change.panes,
            [
                Pane {
                    id: PaneId(70000),
                    left: 0,
                    top: 0,
                    cols: 39,
                    rows: 24
                },
                Pane {
                    id: PaneId(2),
                    left: 40,
                    top: 0,
                    cols: 40,
                    rows: 11
                },
                Pane {
                    id: PaneId(3),
                    left: 40,
                    top: 12,
                    cols: 40,
                    rows: 12
                },
            ]
        );
    }

    #[test]
    fn server_layout_matches_pane_geometry_and_zoom() {
        use tmux_workspaces_test_support::TmuxServer;
        let server = TmuxServer::start().unwrap();
        let session = TmuxServer::SESSION;
        server.run(&["split-window", "-h", "-t", session]).unwrap();
        server.run(&["split-window", "-v", "-t", session]).unwrap();
        let read = || {
            let line = server
                .run(&[
                    "display-message",
                    "-p",
                    "-t",
                    session,
                    "#{window_id} #{window_layout} #{window_visible_layout} #{window_raw_flags}",
                ])
                .unwrap();
            parse(format!("%layout-change {line}").as_bytes()).unwrap()
        };
        let change = read();
        assert_eq!(change.panes.len(), 3);
        for pane in &change.panes {
            let actual = server
                .run(&[
                    "display-message",
                    "-p",
                    "-t",
                    &format!("%{}", pane.id.0),
                    "#{pane_left} #{pane_top} #{pane_width} #{pane_height}",
                ])
                .unwrap();
            assert_eq!(
                actual.trim(),
                format!("{} {} {} {}", pane.left, pane.top, pane.cols, pane.rows)
            );
        }
        let pane = change.panes[1].id;
        server
            .run(&["resize-pane", "-Z", "-t", &format!("%{}", pane.0)])
            .unwrap();
        assert_eq!(
            read().panes,
            [Pane {
                id: pane,
                left: 0,
                top: 0,
                cols: 200,
                rows: 50
            }]
        );
        assert!(parse(b"%layout-change @1 abcd,80x24,0,0,1 abcd,80x24,0,0,1 ").is_ok());
    }

    #[test]
    fn json_layout_preserves_rectangles_and_rejects_floating_panes() {
        let tiled = r#"{"V":2,"L":{"t":"p","w":80,"h":24,"x":0,"y":0,"I":"%70000"}}"#;
        let change = parse(format!("%layout-change @1 {tiled} {tiled} *").as_bytes()).unwrap();
        assert_eq!(
            change.panes,
            [Pane {
                id: PaneId(70000),
                left: 0,
                top: 0,
                cols: 80,
                rows: 24
            }]
        );
        let floating = tiled.replace("\"I\":", "\"z\":1,\"I\":");
        assert!(
            parse(format!("%layout-change @1 {floating} {floating}").as_bytes())
                .unwrap_err()
                .contains("floating")
        );
    }

    #[test]
    fn rejects_truncated_splits_and_out_of_range_rectangles() {
        assert!(parse(b"%layout-change @1 abcd,80x24,0,0,0 abcd,80x24,0,0[80x24,0,0,0").is_err());
        assert!(parse(b"%layout-change @1 abcd,80x24,0,0,0 abcd,65536x24,0,0,0").is_err());
    }
}
