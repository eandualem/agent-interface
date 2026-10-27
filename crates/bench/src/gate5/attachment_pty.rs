//! Ordinary-client attachment alternative.
//! Only owned private fixtures receive input; no desktop terminal is involved.

use std::time::{Duration, Instant};

use agent_interface_term::{Screen, Terminal};
use agent_interface_test_support::TmuxServer;

use super::{connection::TIMEOUT, pty::Reference, session::quote};

#[track_caller]
fn wait_for(mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + TIMEOUT;
    while !ready() {
        assert!(Instant::now() < deadline, "fixture condition timed out");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn describe(server: &TmuxServer, target: &str, format: &str) -> String {
    server
        .run(&["display-message", "-p", "-t", target, format])
        .unwrap()
        .trim_end_matches('\n')
        .to_owned()
}

/// Decode the PTY stream with the native terminal core, retaining parser state.
fn screen_with(client: &Reference, terminal: &mut Terminal, offset: &mut usize, text: &str) {
    let deadline = Instant::now() + TIMEOUT;
    let mut drawn = String::new();
    loop {
        let bytes = client.bytes_from(*offset);
        *offset += bytes.len();
        terminal.write(&bytes);
        let mut screen = Screen::default();
        terminal.read_screen(&mut screen).unwrap();
        drawn.clear();
        for row in 0..screen.rows() {
            for col in 0..screen.cols() {
                drawn.push_str(screen.cell(row, col).unwrap().0);
            }
            drawn.push('\n');
        }
        if drawn.contains(text) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "PTY screen timed out waiting for {text:?}; decoded {} bytes; grid:\n{drawn}",
            *offset
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    eprintln!("native grid contains {text:?}");
}

#[test]
fn ordinary_client_preserves_prefix_modes_and_session_identity() {
    let server = TmuxServer::start().unwrap();
    let files = tempfile::tempdir().unwrap();
    let input = files.path().join("input");
    // The pane records raw application input so prefix/menu keys cannot silently
    // pass through to the program while the tmux-side assertions still succeed.
    server
        .run(&[
            "respawn-pane",
            "-k",
            "-t",
            "fixture:0",
            &format!(
                "stty raw -echo; printf 'PTY attachment ready'; cat > {}",
                quote(input.to_str().unwrap())
            ),
        ])
        .unwrap();
    server
        .run(&["set-option", "-t", "fixture", "destroy-unattached", "off"])
        .unwrap();
    server
        .run(&["new-window", "-d", "-t", "fixture:1", "sleep 1000"])
        .unwrap();
    server
        .run(&["new-session", "-d", "-t", "fixture", "-s", "helper"])
        .unwrap();
    // Automatic helper cleanup is a separate, unresolved lifecycle check.
    server
        .run(&["set-option", "-t", "helper", "destroy-unattached", "off"])
        .unwrap();
    // Synthetic key bursts can trigger tmux's timing-based paste detection.
    // This fixture exercises bindings, not that independent input heuristic.
    server
        .run(&["set-option", "-t", "helper", "assume-paste-time", "0"])
        .unwrap();
    server
        .run(&["set-option", "-g", "status-left", "PTY fixture"])
        .unwrap();
    server
        .run(&["set-option", "-g", "status-right", ""])
        .unwrap();
    server
        .run(&[
            "bind-key",
            "-T",
            "prefix",
            "x",
            "set-option",
            "-t",
            "helper",
            "@prefix-probe",
            "hit",
        ])
        .unwrap();
    server
        .run(&[
            "bind-key",
            "-T",
            "prefix",
            "p",
            "command-prompt",
            "-p",
            "Probe prompt",
            "set-option -t helper @prompt-probe '%%'",
        ])
        .unwrap();
    server
        .run(&[
            "bind-key",
            "-T",
            "prefix",
            "m",
            "display-menu",
            "-T",
            "Probe menu",
            "Choose marker",
            "x",
            "set-option -t helper @menu-probe chosen",
        ])
        .unwrap();
    server
        .run(&[
            "set-environment",
            "-t",
            "helper",
            "DISPLAY",
            "fixture-display",
        ])
        .unwrap();
    let identity = describe(
        &server,
        "fixture:0",
        "#{session_id}|#{window_id}|#{pane_pid}",
    );
    let client = Reference::attach(&server, "helper", 80, 24).unwrap();
    let mut terminal = Terminal::new(80, 24, 0).unwrap();
    let mut offset = 0;
    screen_with(&client, &mut terminal, &mut offset, "PTY attachment ready");
    assert_eq!(
        server
            .run(&["show-environment", "-t", "helper", "DISPLAY"])
            .unwrap()
            .trim(),
        "DISPLAY=fixture-display"
    );
    let option = |name| {
        server
            .run(&["show-options", "-v", "-t", "helper", name])
            .unwrap_or_default()
            .trim()
            .to_owned()
    };

    client.send(b"\x02x").unwrap();
    wait_for(|| option("@prefix-probe") == "hit");
    client.send(b"\x02p").unwrap();
    screen_with(&client, &mut terminal, &mut offset, "Probe prompt");
    client.send(b"answer\r").unwrap();
    wait_for(|| option("@prompt-probe") == "answer");
    client.send(b"\x02m").unwrap();
    screen_with(&client, &mut terminal, &mut offset, "Choose marker");
    client.send(b"x").unwrap();
    wait_for(|| option("@menu-probe") == "chosen");

    client.send(b"\x02[").unwrap();
    wait_for(|| describe(&server, "helper:0", "#{pane_in_mode}") == "1");
    screen_with(&client, &mut terminal, &mut offset, "[0/0]");
    client.send(b"q").unwrap();
    wait_for(|| describe(&server, "helper:0", "#{pane_in_mode}") == "0");
    assert!(std::fs::read(&input).unwrap().is_empty());
    client.send(b"PAYLOAD").unwrap();
    wait_for(|| std::fs::read(&input).unwrap() == b"PAYLOAD");
    client.send(b"\x02\x02").unwrap();
    wait_for(|| std::fs::read(&input).unwrap() == b"PAYLOAD\x02");

    client.send(b"\x02n").unwrap();
    wait_for(|| describe(&server, "helper", "#{window_index}") == "1");
    assert_eq!(describe(&server, "fixture", "#{window_index}"), "0");
    // A custom prompt owns p above; use the unmodified last-window binding.
    client.send(b"\x02l").unwrap();
    wait_for(|| describe(&server, "helper", "#{window_index}") == "0");
    client.send(b"\x02d").unwrap();
    wait_for(|| {
        server
            .run(&["list-clients", "-t", "helper", "-F", "#{client_pid}"])
            .unwrap()
            .is_empty()
    });
    drop(client);
    assert_eq!(
        describe(
            &server,
            "fixture:0",
            "#{session_id}|#{window_id}|#{pane_pid}"
        ),
        identity
    );

    let reconnected = Reference::attach(&server, "helper", 80, 24).unwrap();
    let mut terminal = Terminal::new(80, 24, 0).unwrap();
    screen_with(&reconnected, &mut terminal, &mut 0, "PTY attachment ready");
    reconnected.send(b"AGAIN").unwrap();
    wait_for(|| std::fs::read(&input).unwrap() == b"PAYLOAD\x02AGAIN");
    reconnected.send(b"\x02d").unwrap();
    wait_for(|| {
        server
            .run(&["list-clients", "-t", "helper", "-F", "#{client_pid}"])
            .unwrap()
            .is_empty()
    });
    drop(reconnected);
    server.run(&["kill-session", "-t", "=helper"]).unwrap();
    assert_eq!(
        describe(
            &server,
            "fixture:0",
            "#{session_id}|#{window_id}|#{pane_pid}"
        ),
        identity
    );
    assert_eq!(std::fs::read(&input).unwrap(), b"PAYLOAD\x02AGAIN");
}

/// An outer terminal must answer tmux, while tmux answers the application.
/// Use the engine directly here: the product wrapper intentionally drops
/// replies for its existing control-mode pane stream.
#[test]
#[ignore = "opt-in protocol probe: clipboard crashes tmux 3.3a linked to ncurses 6.6"]
fn ordinary_host_routes_replies_paste_mouse_and_clipboard() {
    use std::cell::RefCell;

    use libghostty_vt::terminal::{
        ConformanceLevel, DeviceAttributes, DeviceType, Mode, Options, PrimaryDeviceAttributes,
        SecondaryDeviceAttributes, Terminal as Host, TertiaryDeviceAttributes,
    };
    use libghostty_vt::{key, mouse};

    let server = TmuxServer::start().unwrap();
    let files = tempfile::tempdir().unwrap();
    let input = files.path().join("input");
    server.run(&["set-option", "-g", "status", "off"]).unwrap();
    server.run(&["set-option", "-g", "mouse", "on"]).unwrap();
    server
        .run(&["set-option", "-g", "assume-paste-time", "0"])
        .unwrap();
    server.run(&["respawn-pane", "-k", "-t", "fixture:0", &format!(
        "stty raw -echo; printf 'HOST_READY\\033[6n\\033[?2004h\\033[?1000h\\033[?1006h'; cat > {}",
        quote(input.to_str().unwrap())
    )]).unwrap();
    let replies = RefCell::new(Vec::new());
    let clipboard = RefCell::new(Vec::new());
    let mut host = Host::new(Options {
        cols: 80,
        rows: 24,
        max_scrollback: 0,
    })
    .unwrap();
    host.on_pty_write(|_, data| replies.borrow_mut().extend_from_slice(data))
        .unwrap();
    host.on_device_attributes(|_| {
        Some(DeviceAttributes {
            primary: PrimaryDeviceAttributes::new(ConformanceLevel::VT220, &[]),
            secondary: SecondaryDeviceAttributes {
                device_type: DeviceType::VT220,
                firmware_version: 0,
                rom_cartridge: 0,
            },
            tertiary: TertiaryDeviceAttributes::default(),
        })
    })
    .unwrap();
    host.on_clipboard_write(|_, write| {
        for content in write.contents() {
            clipboard
                .borrow_mut()
                .push((content.mime.to_owned(), content.data.to_owned()));
        }
        Ok(())
    })
    .unwrap();
    let client = Reference::attach(&server, "fixture", 80, 24).unwrap();
    let mut offset = 0;
    let mut returned = Vec::new();
    let mut pump = |host: &mut Host<'_, '_>| {
        let bytes = client.bytes_from(offset);
        offset += bytes.len();
        host.vt_write(&bytes);
        let bytes = std::mem::take(&mut *replies.borrow_mut());
        client.send(&bytes).unwrap();
        returned.extend(bytes);
    };
    wait_for(|| {
        pump(&mut host);
        host.mode(Mode::BRACKETED_PASTE).unwrap() && host.mode(Mode::SGR_MOUSE).unwrap()
    });
    // The application asked once. Outer-terminal negotiation must not leak
    // a second response (or any other bytes) into its raw input.
    wait_for(|| std::fs::read(&input).unwrap_or_default() == b"\x1b[1;11R");

    let mut pasted = *b"one\ntwo";
    let mut encoded = [0; 64];
    let size = libghostty_vt::paste::encode(
        &mut pasted,
        host.mode(Mode::BRACKETED_PASTE).unwrap(),
        &mut encoded,
    )
    .unwrap();
    client.send(&encoded[..size]).unwrap();
    let mut expected = b"\x1b[1;11R\x1b[200~one\ntwo\x1b[201~".to_vec();
    wait_for(|| std::fs::read(&input).unwrap() == expected);

    // Protocol input only: no desktop mouse event or clipboard API is used.
    let mut mouse = mouse::Encoder::new().unwrap();
    mouse
        .set_options_from_terminal(&host)
        .set_size(mouse::EncoderSize {
            screen_width: 800,
            screen_height: 480,
            cell_width: 10,
            cell_height: 20,
            padding_top: 0,
            padding_bottom: 0,
            padding_left: 0,
            padding_right: 0,
        });
    let mut event = mouse::Event::new().unwrap();
    event
        .set_action(mouse::Action::Press)
        .set_button(Some(mouse::Button::Left))
        .set_position(mouse::Position { x: 20.0, y: 20.0 });
    let size = mouse.encode(&event, &mut encoded).unwrap();
    assert_eq!(&encoded[..size], b"\x1b[<0;3;2M");
    client.send(&encoded[..size]).unwrap();
    expected.extend_from_slice(b"\x1b[<0;3;2M");
    wait_for(|| std::fs::read(&input).unwrap() == expected);
    let mut keys = key::Encoder::new().unwrap();
    keys.set_options_from_terminal(&host);
    let mut event = key::Event::new().unwrap();
    event
        .set_action(key::Action::Press)
        .set_key(key::Key::ArrowUp);
    let size = keys.encode(&event, &mut encoded).unwrap();
    client.send(&encoded[..size]).unwrap();
    expected.extend_from_slice(b"\x1b[A");
    wait_for(|| std::fs::read(&input).unwrap() == expected);
    server
        .run(&["set-buffer", "-w", "fixture clipboard"])
        .unwrap();
    wait_for(|| {
        pump(&mut host);
        clipboard
            .borrow()
            .iter()
            .any(|(_, data)| data == "fixture clipboard")
    });
    assert!(
        !returned.is_empty(),
        "outer-terminal queries produced no replies"
    );
    client.send(b"END").unwrap();
    expected.extend_from_slice(b"END");
    wait_for(|| std::fs::read(&input).unwrap() == expected);
}

/// Compare a late ordinary attach with an uninterrupted client, using the
/// same fixtures that expose state lost by capture-based recovery.
#[test]
fn ordinary_late_attach_preserves_unexported_pane_state() {
    use super::session::{Layout, SESSION, Session, quiet};
    use super::{compare, fixtures};

    let version = {
        let server = TmuxServer::start().unwrap();
        fixtures::version(
            &server
                .run(&["display-message", "-p", "#{version}"])
                .unwrap(),
        )
        .unwrap()
    };
    for name in [
        "background-erase",
        "tabs",
        "tab-moved",
        "link-wrap",
        "current-style",
        "partial-utf8",
        "saved-cursor",
        "still-line-drawing",
        "alternate-resize",
    ] {
        let fixture = fixtures::FIXTURES
            .iter()
            .find(|fixture| fixture.name == name)
            .unwrap();
        let session = Session::start(version).unwrap();
        let script = session.script(fixture.draw, fixture.more).unwrap();
        session.open(Layout::One(40, 8), &script).unwrap();
        let uninterrupted = Reference::attach(&session.server, SESSION, 40, 8).unwrap();
        let rules = session.reference_ready(&uninterrupted).unwrap();
        session.signal("ref").unwrap();
        session.wait("drawn", TIMEOUT).unwrap();
        quiet(&uninterrupted, None).unwrap();
        let (cols, reference_from) = if name == "alternate-resize" {
            // The pane keeps its saved primary screen at the old width while
            // its alternate screen shrinks. Compare from the ensuing redraw.
            let from = uninterrupted.len();
            uninterrupted.resize(30, 8).unwrap();
            wait_for(|| {
                session
                    .run(&["display-message", "-p", "-t", SESSION, "#{pane_width}"])
                    .unwrap()
                    .trim()
                    == "30"
            });
            quiet(&uninterrupted, None).unwrap();
            (30, from)
        } else {
            (40, 0)
        };
        // Keep the pre-attach display: attaching another client may redraw
        // the first one, which must not hide an initial state difference.
        let before_attach = uninterrupted.bytes_from(reference_from);
        let late = Reference::attach(&session.server, SESSION, cols, 8).unwrap();
        wait_for(|| {
            session
                .run(&["list-clients", "-t", SESSION, "-F", "#{client_pid}"])
                .unwrap()
                .lines()
                .count()
                == 2
        });
        eprintln!(
            "{name}: {}",
            session
                .run(&[
                    "display-message",
                    "-p",
                    "-t",
                    SESSION,
                    "#{pid}|#{socket_path}|#{version}"
                ])
                .unwrap()
                .trim()
        );
        eprintln!(
            "clients: {}",
            session
                .run(&[
                    "list-clients",
                    "-t",
                    SESSION,
                    "-F",
                    "#{client_pid}|#{client_termname}|#{client_termfeatures}|#{client_width}x#{client_height}"
                ])
                .unwrap()
                .trim()
        );
        for continued in [false, true] {
            if continued {
                session.signal("go").unwrap();
                session.wait("more", TIMEOUT).unwrap();
            }
            quiet(&uninterrupted, None).unwrap();
            quiet(&late, None).unwrap();
            let decode = |bytes: &[u8]| {
                let mut terminal = Terminal::new(cols, 8, 0).unwrap();
                terminal.write(bytes);
                compare::grid(&mut terminal).unwrap()
            };
            let reference = decode(&if continued {
                uninterrupted.bytes_from(reference_from)
            } else {
                before_attach.clone()
            });
            let candidate = decode(&late.bytes_from(0));
            assert_eq!(candidate.size(), reference.size());
            let differences = compare::cells(&reference, (0, 0), &candidate, rules);
            assert!(
                differences.is_empty(),
                "{name}, continued={continued}: {differences:?}"
            );
            assert_eq!(
                candidate.cursor, reference.cursor,
                "{name}, continued={continued}"
            );
        }
    }
}

/// The outer terminal's key protocol and the application's protocol are
/// separate: tmux translates between them according to the pane's mode.
#[test]
fn ordinary_host_uses_tmux_extended_key_policy() {
    use std::cell::RefCell;

    use libghostty_vt::key;
    use libghostty_vt::terminal::{Options, Terminal as Host};

    let server = TmuxServer::start().unwrap();
    let version = super::fixtures::version(&describe(&server, "fixture", "#{version}")).unwrap();
    let files = tempfile::tempdir().unwrap();
    let input = files.path().join("input");
    server.run(&["set-option", "-g", "status", "off"]).unwrap();
    server
        .run(&["set-option", "-g", "assume-paste-time", "0"])
        .unwrap();
    server
        .run(&["set-option", "-s", "extended-keys", "on"])
        .unwrap();
    server
        .run(&[
            "set-option",
            "-as",
            "terminal-features",
            "xterm-256color:extkeys",
        ])
        .unwrap();
    let script = files.path().join("keys.sh");
    std::fs::write(&script, format!(
        "stty raw -echo\ncat < /dev/tty > {} &\nprintf '\\033[>4;2mKEYS_READY'\n{}\nprintf '\\033[>4;0mKEYS_DISABLED'\nwait\n",
        quote(input.to_str().unwrap()), server.shell_command(&["wait-for", "keys-disable"])
    )).unwrap();
    server
        .run(&[
            "respawn-pane",
            "-k",
            "-t",
            "fixture:0",
            &format!("sh {}", quote(script.to_str().unwrap())),
        ])
        .unwrap();
    let replies = RefCell::new(Vec::new());
    let mut host = Host::new(Options {
        cols: 80,
        rows: 24,
        max_scrollback: 0,
    })
    .unwrap();
    host.on_pty_write(|_, data| replies.borrow_mut().extend_from_slice(data))
        .unwrap();
    let client = Reference::attach(&server, "fixture", 80, 24).unwrap();
    eprintln!(
        "extended keys: {}",
        describe(&server, "fixture", "#{pid}|#{socket_path}|#{version}")
    );
    let mut offset = 0;
    let mut pump = |host: &mut Host<'_, '_>| {
        let bytes = client.bytes_from(offset);
        offset += bytes.len();
        host.vt_write(&bytes);
        client
            .send(&std::mem::take(&mut *replies.borrow_mut()))
            .unwrap();
    };
    wait_for(|| {
        pump(&mut host);
        let request = if version < (3, 5) {
            b"\x1b[>4;1m"
        } else {
            b"\x1b[>4;2m"
        };
        client
            .bytes_from(0)
            .windows(request.len())
            .any(|bytes| bytes == request)
            && server
                .run(&["capture-pane", "-p", "-t", "fixture:0"])
                .unwrap()
                .contains("KEYS_READY")
            && input.exists()
    });
    assert!(std::fs::read(&input).unwrap().is_empty());
    eprintln!(
        "clients: {}",
        server
            .run(&[
                "list-clients",
                "-t",
                "fixture",
                "-F",
                "#{client_pid}|#{client_termname}|#{client_termfeatures}"
            ])
            .unwrap()
            .trim()
    );
    let mut keys = key::Encoder::new().unwrap();
    let mut encoded = [0; 64];
    let mut expected = Vec::new();
    let expect_input = |expected: &[u8]| {
        let deadline = Instant::now() + TIMEOUT;
        loop {
            let actual = std::fs::read(&input).unwrap();
            if actual == expected {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "input {actual:?}, expected {expected:?}"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    };
    let formats = if version < (3, 5) {
        &["csi-u"][..]
    } else {
        &["csi-u", "xterm"][..]
    };
    for format in formats {
        if version >= (3, 5) {
            server
                .run(&["set-option", "-s", "extended-keys-format", format])
                .unwrap();
        }
        pump(&mut host);
        keys.set_options_from_terminal(&host);
        for (key, mods, text, unshifted, outer, csi) in [
            (
                key::Key::I,
                key::Mods::CTRL,
                "i",
                'i',
                if version < (3, 5) {
                    b"\x1b[105;5u".as_slice()
                } else {
                    b"\x1b[27;5;105~".as_slice()
                },
                if version < (3, 5) {
                    b"\t".as_slice()
                } else {
                    b"\x1b[105;5u".as_slice()
                },
            ),
            (
                key::Key::A,
                key::Mods::CTRL | key::Mods::SHIFT,
                "A",
                'a',
                if version < (3, 5) {
                    b"\x1b[97;6u".as_slice()
                } else {
                    b"\x1b[27;6;65~".as_slice()
                },
                b"\x1b[65;6u".as_slice(),
            ),
        ] {
            let mut event = key::Event::new().unwrap();
            event
                .set_action(key::Action::Press)
                .set_key(key)
                .set_mods(mods)
                .set_utf8(Some(text))
                .set_unshifted_codepoint(unshifted);
            let size = keys.encode(&event, &mut encoded).unwrap();
            assert_eq!(&encoded[..size], outer);
            client.send(&encoded[..size]).unwrap();
            expected.extend_from_slice(if *format == "xterm" { outer } else { csi });
            expect_input(&expected);
        }
        // A literal Tab stays distinct from Ctrl-I on tmux >= 3.5. Earlier
        // versions normalize Ctrl-I to Tab even with extended keys enabled.
        let mut tab = key::Event::new().unwrap();
        tab.set_action(key::Action::Press).set_key(key::Key::Tab);
        let size = keys.encode(&tab, &mut encoded).unwrap();
        assert_eq!(&encoded[..size], b"\t");
        client.send(&encoded[..size]).unwrap();
        expected.push(b'\t');
        expect_input(&expected);
    }
    server.run(&["wait-for", "-S", "keys-disable"]).unwrap();
    wait_for(|| {
        server
            .run(&["capture-pane", "-p", "-t", "fixture:0"])
            .unwrap()
            .contains("KEYS_DISABLED")
    });
    pump(&mut host);
    keys.set_options_from_terminal(&host);
    let mut control_i = key::Event::new().unwrap();
    control_i
        .set_action(key::Action::Press)
        .set_key(key::Key::I)
        .set_mods(key::Mods::CTRL)
        .set_utf8(Some("i"))
        .set_unshifted_codepoint('i');
    let size = keys.encode(&control_i, &mut encoded).unwrap();
    assert_eq!(
        &encoded[..size],
        if version < (3, 5) {
            b"\x1b[105;5u".as_slice()
        } else {
            b"\x1b[27;5;105~".as_slice()
        }
    );
    client.send(&encoded[..size]).unwrap();
    expected.push(b'\t');
    client.send(b"END").unwrap();
    expected.extend_from_slice(b"END");
    expect_input(&expected);
}

/// Keep one native host alive across PTY size changes and compare it with a
/// clean decoder of each resize redraw, including subsequent pane output.
#[test]
fn ordinary_host_retains_state_through_resizing() {
    use std::cell::RefCell;

    use libghostty_vt::render::{CellIterator, RenderState, RowIterator};
    use libghostty_vt::screen::CellContentTag;
    use libghostty_vt::style::StyleColor;
    use libghostty_vt::terminal::{Options, Terminal as Host};

    use super::fixtures;
    use super::session::{Layout, SESSION, Session, quiet};

    let snapshot = |host: &Host<'_, '_>| {
        let mut render = RenderState::new().unwrap();
        let view = render.update(host).unwrap();
        let mut rows = RowIterator::new().unwrap();
        let mut cells = CellIterator::new().unwrap();
        let mut content = Vec::new();
        let mut iter = rows.update(&view).unwrap();
        while let Some(row) = iter.next() {
            let mut iter = cells.update(row).unwrap();
            while let Some(cell) = iter.next() {
                let raw = cell.raw_cell().unwrap();
                let mut text = String::new();
                if raw.has_text().unwrap() {
                    cell.graphemes_utf8(&mut text).unwrap();
                }
                let mut style = cell.style().unwrap();
                match raw.content_tag().unwrap() {
                    CellContentTag::BgColorPalette => {
                        style.bg_color = StyleColor::Palette(raw.bg_color_palette().unwrap());
                    }
                    CellContentTag::BgColorRgb => {
                        style.bg_color = StyleColor::Rgb(raw.bg_color_rgb().unwrap());
                    }
                    _ => {}
                }
                content.push((text, raw.wide().unwrap(), style));
            }
        }
        (
            view.cols().unwrap(),
            view.rows().unwrap(),
            content,
            host.cursor_x().unwrap(),
            host.cursor_y().unwrap(),
            host.is_cursor_visible().unwrap(),
        )
    };
    let version = {
        let server = TmuxServer::start().unwrap();
        fixtures::version(&describe(&server, "fixture", "#{version}")).unwrap()
    };
    for name in ["reflow", "background-erase", "alternate-resize"] {
        let fixture = fixtures::FIXTURES.iter().find(|f| f.name == name).unwrap();
        let session = Session::start(version).unwrap();
        let script = session.script(fixture.draw, fixture.more).unwrap();
        session.open(Layout::One(40, 8), &script).unwrap();
        let client = Reference::attach(&session.server, SESSION, 40, 8).unwrap();
        session.reference_ready(&client).unwrap();
        session.signal("ref").unwrap();
        session.wait("drawn", TIMEOUT).unwrap();
        quiet(&client, None).unwrap();
        let replies = RefCell::new(Vec::new());
        let mut host = Host::new(Options {
            cols: 40,
            rows: 8,
            max_scrollback: 0,
        })
        .unwrap();
        host.on_pty_write(|_, data| replies.borrow_mut().extend_from_slice(data))
            .unwrap();
        let mut offset = 0;
        let mut pump = |host: &mut Host<'_, '_>| {
            let bytes = client.bytes_from(offset);
            offset += bytes.len();
            host.vt_write(&bytes);
            client
                .send(&std::mem::take(&mut *replies.borrow_mut()))
                .unwrap();
        };
        pump(&mut host);
        quiet(&client, None).unwrap();
        pump(&mut host);
        eprintln!(
            "{name}: {}",
            describe(
                &session.server,
                SESSION,
                "#{pid}|#{socket_path}|#{version}|#{pane_pid}"
            )
        );
        for (cols, rows) in [(30, 6), (50, 10)] {
            // The app is waiting at a fixture barrier: drain old-size output
            // before resizing the host and then notifying the owned PTY.
            quiet(&client, None).unwrap();
            pump(&mut host);
            let from = client.len();
            host.resize(cols, rows, 10, 20).unwrap();
            client.resize(cols, rows).unwrap();
            wait_for(|| {
                describe(&session.server, SESSION, "#{pane_width}x#{pane_height}")
                    == format!("{cols}x{rows}")
            });
            quiet(&client, None).unwrap();
            pump(&mut host);
            let mut clean = Host::new(Options {
                cols,
                rows,
                max_scrollback: 0,
            })
            .unwrap();
            clean.vt_write(&client.bytes_from(from));
            assert_eq!(
                snapshot(&host),
                snapshot(&clean),
                "{name}: resize to {cols}x{rows}"
            );
            if cols == 30 {
                let from = client.len();
                session.signal("go").unwrap();
                session.wait("more", TIMEOUT).unwrap();
                quiet(&client, None).unwrap();
                pump(&mut host);
                clean.vt_write(&client.bytes_from(from));
                assert_eq!(
                    snapshot(&host),
                    snapshot(&clean),
                    "{name}: continued output at {cols}x{rows}"
                );
            }
        }
    }
}
