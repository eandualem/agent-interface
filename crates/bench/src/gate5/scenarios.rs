//! Gate 5's scenarios (`docs/BENCHMARKING.md`, harness
//! for gate 5).

use std::path::Path;
use std::time::{Duration, Instant};

use tmux_workspaces_tmux::capture;
use tmux_workspaces_tmux::command::send_bytes;
use tmux_workspaces_tmux::control::PaneId;
use tmux_workspaces_tmux::recovery::{self, Attach, Recovery, Snapshot, Step};

use super::candidate::{self, Candidate};
use super::connection::{Connection, Event, TIMEOUT, lines};
use super::fixtures::{Fixture, Runs};
use super::pty::Reference;
use super::session::{Layout, SESSION, Session, quiet};
use super::{Check, Compared, Comparison};

/// How the candidate rebuilds the fixture's pane.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rebuild {
    /// On a first attach, on a connection without output.
    Attach,
    /// On a connection receiving output, after a pause forced while a
    /// neighbouring pane floods output and the connection stops reading.
    Pause,
}

/// The size of a window with one fixture pane.
const ONE: Layout = Layout::One(40, 8);

/// How long a fixture may take to draw.
const DRAW_TIMEOUT: Duration = Duration::from_secs(20);

/// How long the connection stops reading before and after a forced pause.
const STALL: Duration = Duration::from_millis(100);

/// How long the hidden window floods output around a forced pause.
const FLOOD_SECONDS: f64 = 0.5;

/// The input sent through the candidate's connection after more output.
const INPUT: &[u8] = b"input\r";

/// Whether `fixture` runs when rebuilt by attach or pause in `layout`.
pub fn runs(fixture: &Fixture, layout: Layout) -> bool {
    match fixture.runs {
        Runs::Everywhere => true,
        Runs::OnePane => layout != Layout::Split,
        Runs::Resize | Runs::ResizeWhileDrawing => false,
    }
}

/// Runs `fixture` in `layout`, rebuilt by `rebuild`, and compares after the
/// rebuild and after more output and input.
pub fn fixture(
    version: (u32, u32),
    fixture: &Fixture,
    layout: Layout,
    rebuild: Rebuild,
) -> Result<[Comparison; 2], String> {
    let session = Session::start(version)?;
    let script = session.script(fixture.draw, fixture.more)?;
    let pane = session.open(layout, &script)?;
    if rebuild == Rebuild::Pause {
        session.flood(FLOOD_SECONDS)?;
    }
    let size = layout.window();
    let reference = attach_reference(&session, size)?;
    let rules = session.reference_ready(&reference)?;
    // A connection that receives output is attached before the fixture
    // draws, as the app's would be.
    let receiving = match rebuild {
        Rebuild::Attach => None,
        Rebuild::Pause => Some(Connection::attach(
            &session.server,
            &["attach-session", "-f", "ignore-size", "-t", SESSION],
            &[pane],
        )?),
    };
    session.signal("ref")?;
    session.wait("drawn", DRAW_TIMEOUT)?;
    quiet(&reference, receiving.as_ref())?;
    let (connection, mut candidate) = match receiving {
        None => {
            let (connection, candidates) = first_attach(&session, &[pane])?;
            let candidate = candidates
                .into_iter()
                .next()
                .ok_or("the attach rebuilt no pane")?;
            (connection, candidate)
        }
        Some(connection) => {
            let candidate = forced_pause(&session, &connection, pane)?;
            (connection, candidate)
        }
    };
    quiet(&reference, Some(&connection))?;
    let rebuilt = session.compare(&reference, 0, size, &mut candidate, &connection, rules)?;
    let more = more_and_input(&session, &reference, &connection, pane)
        .and_then(|()| session.compare(&reference, 0, size, &mut candidate, &connection, rules))?;
    Ok([rebuilt, more])
}

/// Runs `fixture` while the reference's terminal is resized four times,
/// rebuilding the pane once each new size settles, and compares at each
/// size if the fixture has stopped drawing.
pub fn resize(version: (u32, u32), fixture: &Fixture, out: &Path) -> Result<Vec<Compared>, String> {
    const STEPS: [(u16, u16); 4] = [(30, 8), (50, 10), (40, 6), (36, 9)];
    let session = Session::start(version)?;
    let script = session.script(fixture.draw, fixture.more)?;
    let pane = session.open(ONE, &script)?;
    let reference = attach_reference(&session, ONE.window())?;
    let rules = session.reference_ready(&reference)?;
    session.signal("ref")?;
    let drawing = fixture.runs == Runs::ResizeWhileDrawing;
    if !drawing {
        session.wait("drawn", DRAW_TIMEOUT)?;
        quiet(&reference, None)?;
    }
    let (connection, candidates) = first_attach(&session, &[pane])?;
    let mut candidate = candidates.into_iter().next().ok_or("no pane rebuilt")?;
    let mut from = 0;
    let mut size = ONE.window();
    let mut notes = Vec::new();
    let mut compared = Vec::new();
    let mut last_mark = 0;
    for step in STEPS {
        let mark = connection.mark();
        last_mark = mark;
        from = reference.len();
        reference
            .resize(step.0, step.1)
            .map_err(|error| error.to_string())?;
        size = step;
        connection.wait_for(mark, TIMEOUT, is_layout_change)?;
        // Rebuild once no further change has arrived for a while.
        loop {
            let mark = connection.mark();
            std::thread::sleep(Duration::from_millis(100));
            if !connection.events_from(mark).iter().any(is_layout_change) {
                break;
            }
        }
        let (rebuilt, next) = pause_and_rebuild(&connection, pane, None)?;
        let state = &rebuilt.snapshot.state;
        let rebuilt_size = (state.width, state.height);
        candidate = Candidate::rebuilt(&rebuilt.snapshot, &rebuilt.output, next)?;
        // A fixture that has stopped drawing is compared at every size; one
        // still drawing only once it stops.
        if !drawing {
            quiet(&reference, Some(&connection))?;
            let mut comparison =
                session.compare(&reference, from, size, &mut candidate, &connection, rules)?;
            comparison.notes.extend(size_note(rebuilt_size, step));
            resize_trace(
                &out.join(format!("{}x{}", step.0, step.1)),
                &candidate,
                &reference,
                from,
                &connection,
                mark,
                &comparison,
            )?;
            compared.push(Compared::rebuilt(Some(step), comparison));
        } else if let Some(note) = size_note(rebuilt_size, step) {
            notes.push(note);
        }
    }
    if drawing {
        std::fs::write(session.stop_file(), b"").map_err(|error| error.to_string())?;
        session.wait("drawn", DRAW_TIMEOUT)?;
        quiet(&reference, Some(&connection))?;
        let mut rebuilt =
            session.compare(&reference, from, size, &mut candidate, &connection, rules)?;
        rebuilt.notes.extend(notes);
        resize_trace(
            &out.join("rebuilt"),
            &candidate,
            &reference,
            from,
            &connection,
            last_mark,
            &rebuilt,
        )?;
        compared.push(Compared::rebuilt(Some(size), rebuilt));
    }
    let more = more_and_input(&session, &reference, &connection, pane).and_then(|()| {
        session.compare(&reference, from, size, &mut candidate, &connection, rules)
    })?;
    resize_trace(
        &out.join("more"),
        &candidate,
        &reference,
        from,
        &connection,
        last_mark,
        &more,
    )?;
    compared.push(Compared::more(more));
    Ok(compared)
}

/// Retains the state and exact byte streams behind a resize comparison.
/// The reference bytes start just before the latest resize, as the oracle
/// does; the directory name or comparison's enclosing report gives its size.
fn resize_trace(
    out: &Path,
    candidate: &Candidate,
    reference: &Reference,
    from: usize,
    connection: &Connection,
    mark: usize,
    comparison: &Comparison,
) -> Result<(), String> {
    std::fs::create_dir_all(out).map_err(|error| error.to_string())?;
    for (name, bytes) in [
        (
            "snapshot.txt",
            format!("{:#?}", candidate.seed).into_bytes(),
        ),
        ("replayed.bin", candidate.replayed.clone()),
        ("reference.bin", reference.bytes_from(from)),
        (
            "events.txt",
            format!("{:#?}", connection.events_from(mark)).into_bytes(),
        ),
        ("comparison.txt", format!("{comparison:#?}").into_bytes()),
    ] {
        std::fs::write(out.join(name), bytes).map_err(|error| error.to_string())?;
    }
    Ok(())
}

/// Notes a rebuild whose size differs from the size the window was given.
fn size_note(rebuilt: (u16, u16), step: (u16, u16)) -> Option<String> {
    (rebuilt != step).then(|| {
        format!(
            "rebuilt at {}x{} after resizing to {}x{}",
            rebuilt.0, rebuilt.1, step.0, step.1
        )
    })
}

/// Flow control: rebuilds a pane printing a count as fast as it can, again
/// and again under backlog, checking that each rebuild continues the count
/// with nothing lost or applied twice; then compares the last rebuild after
/// the count ends, and again after more output and input.
pub fn flow(version: (u32, u32), seconds: f64) -> Result<(Vec<Check>, [Comparison; 2]), String> {
    let layout = Layout::One(80, 24);
    let session = Session::start(version)?;
    let script = session.script(&count(seconds)?, r"printf 'end\n'")?;
    let pane = session.open(layout, &script)?;
    session.flood(seconds)?;
    let reference = attach_reference(&session, layout.window())?;
    let rules = session.reference_ready(&reference)?;
    let connection = Connection::attach(
        &session.server,
        &["attach-session", "-f", "ignore-size", "-t", SESSION],
        &[pane],
    )?;
    session.signal("flood")?;
    session.signal("ref")?;
    let start = Instant::now();
    let mut checks = Vec::new();
    let mut candidate = None;
    // Stop rebuilding a second before the count ends, so that each rebuild
    // is followed by enough output to check.
    while start.elapsed().as_secs_f64() < seconds - 1.0 {
        let (rebuilt, next) = pause_and_rebuild(&connection, pane, Some(STALL))?;
        let mut rebuilt_pane = Candidate::rebuilt(&rebuilt.snapshot, &rebuilt.output, next)?;
        std::thread::sleep(Duration::from_millis(200));
        rebuilt_pane.catch_up(&connection)?;
        checks.push(Check {
            name: format!("rebuild {} continues the count", checks.len() + 1),
            result: continuity(&rebuilt.snapshot, &rebuilt_pane.applied),
        });
        candidate = Some(rebuilt_pane);
    }
    let mut candidate = candidate.ok_or("the count ended before the first rebuild")?;
    session.wait("drawn", DRAW_TIMEOUT)?;
    quiet(&reference, Some(&connection))?;
    let size = layout.window();
    let rebuilt = session.compare(&reference, 0, size, &mut candidate, &connection, rules)?;
    let more = more_and_input(&session, &reference, &connection, pane)
        .and_then(|()| session.compare(&reference, 0, size, &mut candidate, &connection, rules))?;
    Ok((checks, [rebuilt, more]))
}

/// tmux's own pause: the connection sets `pause-after`, then stops reading
/// for 1.5 s at a time while a pane prints a count. After each stall that
/// ends in a pause, the pane is rebuilt and must continue the count. The
/// last rebuild is compared after the count ends, and again after more
/// output and input.
pub fn own_pause(version: (u32, u32), stalls: usize) -> Result<OwnPause, String> {
    let layout = Layout::One(80, 24);
    let session = Session::start(version)?;
    #[allow(clippy::cast_precision_loss)]
    let seconds = stalls as f64 * 3.0 + 5.0;
    let script = session.script(&count(seconds)?, r"printf 'end\n'")?;
    let pane = session.open(layout, &script)?;
    session.flood(seconds)?;
    let reference = attach_reference(&session, layout.window())?;
    let rules = session.reference_ready(&reference)?;
    let connection = Connection::attach(
        &session.server,
        &["attach-session", "-f", "ignore-size", "-t", SESSION],
        &[pane],
    )?;
    command(&connection, "refresh-client -f pause-after=1\n", 1)?;
    session.signal("flood")?;
    session.signal("ref")?;
    // The count ends `seconds` after it starts.
    let deadline = Instant::now() + Duration::from_secs_f64(seconds) + DRAW_TIMEOUT;
    std::thread::sleep(Duration::from_millis(300));
    let ours = format!("%pause %{}", pane.0);
    let is_ours =
        |event: &Event| matches!(event, Event::Notification(line) if *line == ours.as_bytes());
    let mut checks = Vec::new();
    let mut paused = 0;
    let mut candidate = None;
    // Every pause of the pane before event `since` has been handled.
    let mut since = connection.mark();
    let next_pause = |since: usize| {
        connection
            .events_from(since)
            .iter()
            .position(is_ours)
            .map(|offset| since + offset)
    };
    for _ in 0..stalls {
        connection.stall(true);
        std::thread::sleep(Duration::from_millis(1_500));
        connection.stall(false);
        let at = connection.wait_for(since, Duration::from_secs(1), is_ours);
        let others = paused_panes(&connection, since, pane);
        let Ok(at) = at else {
            // tmux pauses a pane only when it queues new output for it and
            // finds the oldest too old; a stalled client often slows the
            // panes instead. A pause of the pane after the wait is found
            // by the next one, or after the last stall.
            continue_panes(&connection, &others)?;
            std::thread::sleep(Duration::from_millis(300));
            continue;
        };
        paused += 1;
        let (snapshot, mut rebuilt) = rebuild_paused(&connection, pane, at)?;
        // The pane runs again only from the rebuild's last command: any
        // later pause is a new one.
        since = at + 1;
        continue_panes(&connection, &others)?;
        std::thread::sleep(Duration::from_millis(200));
        rebuilt.catch_up(&connection)?;
        checks.push(Check {
            name: format!("rebuild after pause {paused} continues the count"),
            result: continuity(&snapshot, &rebuilt.applied),
        });
        candidate = Some(rebuilt);
    }
    let Some(mut candidate) = candidate else {
        return Ok(OwnPause {
            checks,
            paused,
            comparisons: None,
        });
    };
    // tmux may pause the pane again before the count ends: each such pause
    // is rebuilt like the others, without its own check.
    let mut ended = session.waiter("drawn")?;
    let others_from = since;
    let rebuild_pauses = |candidate: &mut Candidate, since: &mut usize| {
        while let Some(at) = next_pause(*since) {
            *candidate = rebuild_paused(&connection, pane, at)?.1;
            *since = at + 1;
        }
        Ok::<_, String>(())
    };
    while !ended.done(Duration::from_millis(100))? {
        rebuild_pauses(&mut candidate, &mut since)?;
        if Instant::now() >= deadline {
            return Err("the count did not end".to_owned());
        }
    }
    // The count's end is signalled through another client: a reply on the
    // connection brings everything tmux wrote to it before, a last pause
    // included. One more round finds a pause during the wait for quiet.
    for _ in 0..2 {
        command(&connection, "display-message -p ''\n", 1)?;
        rebuild_pauses(&mut candidate, &mut since)?;
        quiet(&reference, Some(&connection))?;
    }
    if next_pause(since).is_some() {
        return Err("tmux paused the pane again after the count ended".to_owned());
    }
    continue_panes(&connection, &paused_panes(&connection, others_from, pane))?;
    let size = layout.window();
    let rebuilt = session.compare(&reference, 0, size, &mut candidate, &connection, rules)?;
    let more = more_and_input(&session, &reference, &connection, pane)
        .and_then(|()| session.compare(&reference, 0, size, &mut candidate, &connection, rules))?;
    Ok(OwnPause {
        checks,
        paused,
        comparisons: Some([rebuilt, more]),
    })
}

/// What the own-pause scenario found.
pub struct OwnPause {
    /// The checks that each rebuild continued the count.
    pub checks: Vec<Check>,
    /// How many stalls ended in a pause.
    pub paused: usize,
    /// The comparisons of the last rebuild, if any stall ended in a pause.
    pub comparisons: Option<[Comparison; 2]>,
}

/// Rebuilds `pane`, which tmux paused at the connection's event `at`.
fn rebuild_paused(
    connection: &Connection,
    pane: PaneId,
    at: usize,
) -> Result<(Snapshot, Candidate), String> {
    let (recovery, line) = Recovery::paused(pane);
    // Every earlier command has had its replies: the rebuild's start here.
    let replies_from = connection.mark();
    connection.send(&line)?;
    // The pane's output after its %pause belongs after the rebuild.
    let (rebuilt, next) = candidate::recover(
        connection,
        at + 1,
        replies_from,
        recovery,
        Instant::now() + TIMEOUT,
    )?
    .ok_or("a command failed")?;
    let candidate = Candidate::rebuilt(&rebuilt.snapshot, &rebuilt.output, next)?;
    Ok((rebuilt.snapshot, candidate))
}

/// The pane exits between the pause and the capture: the rebuild must fail
/// and its replies be discarded.
pub fn exit_after_pause(version: (u32, u32)) -> Result<Check, String> {
    let session = Session::start(version)?;
    // The pane exits once the harness signals it.
    let quit = session.server.shell_command(&["wait-for", "quit"]);
    let pane = session.open(ONE, &quit)?;
    session.split(pane, "sleep 1000")?;
    let connection = Connection::attach(
        &session.server,
        &["attach-session", "-f", "ignore-size", "-t", SESSION],
        &[pane],
    )?;
    let from = connection.mark();
    let (mut recovery, pause) = Recovery::start(pane);
    connection.send(&pause)?;
    let mut index = from;
    let line = loop {
        if let Event::Reply(reply) = connection.next(&mut index, TIMEOUT)? {
            match recovery.reply(reply).map_err(|error| error.to_string())? {
                Step::Send(line) => break line,
                step => return Err(format!("unexpected step {step:?} after the pause")),
            }
        }
    };
    session.signal("quit")?;
    wait_until_gone(&session, pane)?;
    connection.send(&line)?;
    let result = loop {
        if let Event::Reply(reply) = connection.next(&mut index, TIMEOUT)? {
            match recovery.reply(reply).map_err(|error| error.to_string())? {
                Step::Wait => {}
                Step::Failed => break Ok(()),
                step => break Err(format!("the rebuild ended with {step:?}")),
            }
        }
    };
    Ok(Check {
        name: "a pane that exits after its pause fails the rebuild".to_owned(),
        result,
    })
}

/// A pane exits during a first attach while another remains: the line
/// fails, output stays off while the remaining pane writes more, and a
/// second line for the remaining pane on the same connection rebuilds it.
pub fn exit_during_attach(version: (u32, u32)) -> Result<(Check, [Comparison; 2]), String> {
    let session = Session::start(version)?;
    let script = session.script(r"printf 'kept\n\033[1mbold\033[0m'", r"printf ' more'")?;
    let pane = session.open(ONE, &script)?;
    let quit = session.server.shell_command(&["wait-for", "quit"]);
    let leaving = session.split(pane, &quit)?;
    let size = ONE.window();
    let reference = attach_reference(&session, size)?;
    let rules = session.reference_ready(&reference)?;
    session.signal("ref")?;
    session.wait("drawn", DRAW_TIMEOUT)?;
    quiet(&reference, None)?;
    let connection = Connection::attach(
        &session.server,
        &[
            "attach-session",
            "-f",
            "no-output,ignore-size",
            "-t",
            SESSION,
        ],
        &[pane, leaving],
    )?;
    session.signal("quit")?;
    wait_until_gone(&session, leaving)?;
    let from = connection.mark();
    let (attach, line) = Attach::new(vec![pane, leaving]);
    connection.send(&line)?;
    let failed = candidate::attach(&connection, from, attach)?.is_none();
    // The remaining pane writes more: the reference draws it, and the
    // connection must not receive it.
    session.signal("go")?;
    session.wait("more", DRAW_TIMEOUT)?;
    quiet(&reference, None)?;
    let quiet_output = connection.output_bytes() == 0;
    let from = connection.mark();
    let (attach, line) = Attach::new(vec![pane]);
    connection.send(&line)?;
    let (snapshots, next) =
        candidate::attach(&connection, from, attach)?.ok_or("the second attach line failed")?;
    let snapshot = snapshots.first().ok_or("no pane rebuilt")?;
    let mut candidate = Candidate::rebuilt(snapshot, &[], next)?;
    let session_remains = session.run(&["has-session", "-t", SESSION]).is_ok();
    let result = match (failed, quiet_output, session_remains) {
        (true, true, true) => Ok(()),
        _ => Err(format!(
            "line failed {failed}, output stayed off {quiet_output}, session remains {session_remains}"
        )),
    };
    quiet(&reference, Some(&connection))?;
    let rebuilt = session.compare(&reference, 0, size, &mut candidate, &connection, rules)?;
    let more = input(&session, &reference, &connection, pane)
        .and_then(|()| session.compare(&reference, 0, size, &mut candidate, &connection, rules))?;
    Ok((
        Check {
            name: "a pane that exits during a first attach fails the line, and a second line rebuilds the rest".to_owned(),
            result,
        },
        [rebuilt, more],
    ))
}

/// A hook on a command the rebuild sends must be reported.
pub fn hooks(version: (u32, u32)) -> Result<Vec<Check>, String> {
    let session = Session::start(version)?;
    let script = session.script("true", "true")?;
    session.open(ONE, &script)?;
    let connection = Connection::attach(
        &session.server,
        &[
            "attach-session",
            "-f",
            "no-output,ignore-size",
            "-t",
            SESSION,
        ],
        &[],
    )?;
    let reported = || -> Result<bool, String> {
        let replies = command(&connection, &recovery::hooks_command(SESSION), 2)?;
        Ok(recovery::hooks_break_recovery(
            replies
                .iter()
                .flat_map(|reply| reply.lines.iter().map(Vec::as_slice)),
        ))
    };
    let mut checks = Vec::new();
    let mut check = |name: &str, expected: bool, reported: bool| {
        checks.push(Check {
            name: name.to_owned(),
            result: if reported == expected {
                Ok(())
            } else {
                Err(format!("reported {reported}"))
            },
        });
    };
    check("no hooks: not reported", false, reported()?);
    session.run(&[
        "set-hook",
        "-t",
        SESSION,
        "after-display-message",
        "run-shell true",
    ])?;
    check(
        "a session hook on display-message is reported",
        true,
        reported()?,
    );
    session.run(&["set-hook", "-u", "-t", SESSION, "after-display-message"])?;
    session.run(&["set-hook", "-g", "after-capture-pane", "run-shell true"])?;
    check(
        "a global hook on capture-pane is reported",
        true,
        reported()?,
    );
    Ok(checks)
}

fn attach_reference(session: &Session, (cols, rows): (u16, u16)) -> Result<Reference, String> {
    Reference::attach(&session.server, SESSION, cols, rows)
        .map_err(|error| format!("reference client: {error}"))
}

/// Attaches a connection without output and rebuilds `panes` on it.
fn first_attach(
    session: &Session,
    panes: &[PaneId],
) -> Result<(Connection, Vec<Candidate>), String> {
    let connection = Connection::attach(
        &session.server,
        &[
            "attach-session",
            "-f",
            "no-output,ignore-size",
            "-t",
            SESSION,
        ],
        panes,
    )?;
    let from = connection.mark();
    let (attach, line) = Attach::new(panes.to_vec());
    connection.send(&line)?;
    let (snapshots, next) = candidate::attach(&connection, from, attach)?
        .ok_or("a command of the attach line failed")?;
    let candidates = snapshots
        .iter()
        .map(|snapshot| Candidate::rebuilt(snapshot, &[], next))
        .collect::<Result<_, _>>()?;
    Ok((connection, candidates))
}

/// Rebuilds `pane` after a pause forced while the session's hidden window
/// floods output and the connection stops reading around the pause.
fn forced_pause(
    session: &Session,
    connection: &Connection,
    pane: PaneId,
) -> Result<Candidate, String> {
    session.signal("flood")?;
    std::thread::sleep(Duration::from_millis(50));
    let (rebuilt, next) = pause_and_rebuild(connection, pane, Some(STALL))?;
    Candidate::rebuilt(&rebuilt.snapshot, &rebuilt.output, next)
}

/// Pauses `pane` and rebuilds it. With `stall`, the connection stops reading
/// for that long before the pause and after it.
fn pause_and_rebuild(
    connection: &Connection,
    pane: PaneId,
    stall: Option<Duration>,
) -> Result<(recovery::Rebuilt, usize), String> {
    if let Some(stall) = stall {
        connection.stall(true);
        std::thread::sleep(stall);
    }
    let from = connection.mark();
    let (recovery, pause) = Recovery::start(pane);
    let sent = connection.send(&pause);
    if let Some(stall) = stall {
        std::thread::sleep(stall);
        connection.stall(false);
    }
    sent?;
    candidate::recover(connection, from, from, recovery, Instant::now() + TIMEOUT)?
        .ok_or_else(|| "a command of the rebuild failed".to_owned())
}

/// Writes more output from the fixture, then input through the candidate's
/// connection, and waits until both have been drawn.
fn more_and_input(
    session: &Session,
    reference: &Reference,
    connection: &Connection,
    pane: PaneId,
) -> Result<(), String> {
    session.signal("go")?;
    session.wait("more", DRAW_TIMEOUT)?;
    input(session, reference, connection, pane)
}

/// Sends input through the candidate's connection and waits until the
/// fixture has drawn it.
fn input(
    session: &Session,
    reference: &Reference,
    connection: &Connection,
    pane: PaneId,
) -> Result<(), String> {
    let from = connection.mark();
    connection.send(&send_bytes(pane, INPUT))?;
    connection.wait_for(from, TIMEOUT, |event| matches!(event, Event::Reply(_)))?;
    session.wait("input", DRAW_TIMEOUT)?;
    quiet(reference, Some(connection))
}

/// Sends `line` and returns the replies to its `commands` commands.
fn command(
    connection: &Connection,
    line: &str,
    commands: usize,
) -> Result<Vec<recovery::Reply>, String> {
    let mut index = connection.mark();
    connection.send(line)?;
    let mut replies = Vec::new();
    while replies.len() < commands {
        if let Event::Reply(reply) = connection.next(&mut index, TIMEOUT)? {
            if !reply.ok {
                return Err(format!("{}: {}", line.trim_end(), lines(&reply)));
            }
            replies.push(reply);
        }
    }
    Ok(replies)
}

/// Returns the panes other than `pane` that tmux paused since event `from`.
fn paused_panes(connection: &Connection, from: usize, pane: PaneId) -> Vec<PaneId> {
    connection
        .events_from(from)
        .iter()
        .filter_map(|event| match event {
            Event::Notification(line) => line.strip_prefix(b"%pause %"),
            _ => None,
        })
        .filter_map(|number| std::str::from_utf8(number).ok()?.parse().ok().map(PaneId))
        .filter(|&paused| paused != pane)
        .collect()
}

fn continue_panes(connection: &Connection, panes: &[PaneId]) -> Result<(), String> {
    if panes.is_empty() {
        return Ok(());
    }
    let commands: Vec<String> = panes
        .iter()
        .map(|pane| format!("refresh-client -A '%{}:continue'", pane.0))
        .collect();
    command(
        connection,
        &format!("{}\n", commands.join(" ; ")),
        panes.len(),
    )
    .map(drop)
}

fn is_layout_change(event: &Event) -> bool {
    matches!(event, Event::Notification(line) if line.starts_with(b"%layout-change "))
}

fn wait_until_gone(session: &Session, pane: PaneId) -> Result<(), String> {
    let deadline = Instant::now() + TIMEOUT;
    let id = format!("%{}", pane.0);
    while session
        .run(&["list-panes", "-s", "-t", SESSION, "-F", "#{pane_id}"])?
        .lines()
        .any(|line| line == id)
    {
        if Instant::now() >= deadline {
            return Err(format!("{id} did not exit"));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    Ok(())
}

/// The shell command that runs the `count` producer for `seconds`.
fn count(seconds: f64) -> Result<String, String> {
    let program = std::env::current_exe().map_err(|error| error.to_string())?;
    Ok(format!(
        "{} count --seconds {seconds}",
        super::session::quote(&program.display().to_string())
    ))
}

/// Checks that a rebuilt pane continues the `count` producer's numbers:
/// the captured history and screen up to the cursor, then the pending bytes
/// and the output after the rebuild, read as consecutive numbers with none
/// missing or repeated.
fn continuity(snapshot: &Snapshot, after: &[u8]) -> Result<(), String> {
    let state = &snapshot.state;
    let rows: Vec<String> = snapshot.rows.iter().map(text).collect();
    let live: String = String::from_utf8_lossy(&strip_controls(
        &[snapshot.pending.as_slice(), after].concat(),
    ))
    .replace('\r', "");
    let cursor_row = state.history + usize::from(state.cursor.1);
    let row = rows.get(cursor_row).map_or("", String::as_str);
    // Captured between a line's carriage return and its line feed, the
    // cursor is back at the start of a row that still holds its number.
    let row = if live.starts_with('\n') {
        row.trim_end()
    } else {
        &row[..row
            .char_indices()
            .nth(usize::from(state.cursor.0))
            .map_or(row.len(), |(index, _)| index)]
    };
    let mut text = rows[..cursor_row.min(rows.len())].join("\n");
    if cursor_row > 0 {
        text.push('\n');
    }
    text.push_str(row);
    text.push_str(&live);
    let mut numbers = text.split('\n').collect::<Vec<_>>();
    // The last line may not be complete yet.
    numbers.pop();
    let after_rebuild = live.matches('\n').count();
    if after_rebuild < 20 {
        return Err(format!(
            "only {after_rebuild} lines after the rebuild to check"
        ));
    }
    let mut previous: Option<u64> = None;
    for (index, line) in numbers.iter().enumerate() {
        let number: u64 = line
            .trim()
            .parse()
            .map_err(|_| format!("line {index} reads {line:?}"))?;
        if let Some(previous) = previous
            && number != previous + 1
        {
            let kind = if number <= previous {
                "applied twice"
            } else {
                "lost"
            };
            return Err(format!("output {kind}: {number} after {previous}"));
        }
        previous = Some(number);
    }
    Ok(())
}

fn text(row: &capture::Row) -> String {
    row.segments
        .iter()
        .map(|segment| segment.text.as_str())
        .collect()
}

/// Removes the control sequences the `count` producer writes.
fn strip_controls(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut iter = bytes.iter().copied();
    while let Some(byte) = iter.next() {
        if byte != 0x1b {
            out.push(byte);
            continue;
        }
        if iter.next() == Some(b'[') {
            // Parameters, then one final byte.
            for byte in iter.by_ref() {
                if (0x40..=0x7e).contains(&byte) {
                    break;
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use tmux_workspaces_tmux::capture::{Row, Segment, Style};
    use tmux_workspaces_tmux::recovery::{Modes, PaneState};

    use super::*;

    #[test]
    fn recovery_deadline_is_not_renewed_by_each_reply() {
        let session = Session::start((3, 4)).unwrap();
        let pane = session.open(ONE, "sleep 1000").unwrap();
        let (connection, _) = first_attach(&session, &[pane]).unwrap();
        session
            .run(&[
                "set-hook",
                "-g",
                "after-refresh-client",
                "run-shell 'sleep 0.2'",
            ])
            .unwrap();
        let from = connection.mark();
        let (recovery, pause) = Recovery::start(pane);
        let deadline = Instant::now() + Duration::from_millis(50);
        connection.send(&pause).unwrap();
        let result = candidate::recover(&connection, from, from, recovery, deadline);
        assert!(result.unwrap_err().contains("timed out"));
    }

    #[test]
    fn off_grid_cursor_keeps_its_state_until_output_can_be_streamed() {
        for width in [36, 40] {
            for name in [
                "offgrid-print",
                "offgrid-backspace",
                "offgrid-left",
                "offgrid-nowrap",
                "nowrap-margin",
            ] {
                let fixture = super::super::fixtures::FIXTURES
                    .iter()
                    .find(|fixture| fixture.name == name)
                    .unwrap();
                let mut session = Session::start((0, 0)).unwrap();
                let version = session
                    .run(&["display-message", "-p", "#{version}"])
                    .unwrap();
                session.version = super::super::fixtures::version(version.trim()).unwrap();
                let script = session.script(fixture.draw, fixture.more).unwrap();
                let pane = session.open(Layout::One(40, 6), &script).unwrap();
                let reference = attach_reference(&session, (40, 6)).unwrap();
                let rules = session.reference_ready(&reference).unwrap();
                session.signal("ref").unwrap();
                session.wait("drawn", DRAW_TIMEOUT).unwrap();
                quiet(&reference, None).unwrap();
                let (connection, _) = first_attach(&session, &[pane]).unwrap();
                let from = reference.len();
                let mark = connection.mark();
                reference.resize(width, 9).unwrap();
                connection
                    .wait_for(mark, TIMEOUT, is_layout_change)
                    .unwrap();
                quiet(&reference, Some(&connection)).unwrap();
                let (rebuilt, next) = pause_and_rebuild(&connection, pane, None).unwrap();
                let column = if name == "nowrap-margin" { 38 } else { 40 };
                assert_eq!(rebuilt.snapshot.state.cursor, (column, 5));
                let mut candidate =
                    Candidate::rebuilt(&rebuilt.snapshot, &rebuilt.output, next).unwrap();
                for phase in ["rebuilt", "more", "input"] {
                    match phase {
                        "more" => {
                            session.signal("go").unwrap();
                            session.wait("more", DRAW_TIMEOUT).unwrap();
                        }
                        "input" => input(&session, &reference, &connection, pane).unwrap(),
                        _ => {}
                    }
                    quiet(&reference, Some(&connection)).unwrap();
                    let result = session
                        .compare(
                            &reference,
                            from,
                            (width, 9),
                            &mut candidate,
                            &connection,
                            rules,
                        )
                        .unwrap();
                    assert!(result.matches(), "{name} {phase}: {result:?}");
                    if phase == "rebuilt" || (phase == "more" && name == "offgrid-nowrap") {
                        assert_eq!(candidate.cursor().unwrap(), (column, 5));
                        assert!(candidate.cursor_enabled().unwrap());
                    }
                }
            }
        }
    }

    fn snapshot(rows: &[&str], history: usize, cursor: (u16, u16), pending: &[u8]) -> Snapshot {
        Snapshot {
            pane: PaneId(0),
            rows: rows
                .iter()
                .map(|text| Row {
                    segments: vec![Segment {
                        style: Style::default(),
                        link: None,
                        line_drawing: false,
                        text: (*text).to_owned(),
                    }],
                })
                .collect(),
            saved: Vec::new(),
            state: PaneState {
                left: 0,
                top: 0,
                width: 80,
                height: u16::try_from(rows.len() - history).unwrap(),
                history,
                alternate_saved_cursor: None,
                cursor,
                cursor_visible: true,
                scroll_region: (0, 0),
                tab_stops: Vec::new(),
                modes: Modes::default(),
            },
            pending: pending.to_vec(),
        }
    }

    #[test]
    fn an_in_event_margin_transition_preserves_the_exact_replay_prefix() {
        let mut seed = snapshot(&[""], 0, (0, 0), b"");
        seed.state.modes.wrap = true;
        let prefix = vec![b'0'; 80];
        let output = [prefix.as_slice(), b"\x1b[5DX"].concat();
        let candidate = Candidate::rebuilt(&seed, &output, 0).unwrap();
        assert!(candidate.cursor().unwrap_err().contains("new snapshot"));
        assert_eq!(candidate.replayed, prefix);
        assert_eq!(candidate.applied, output);
    }

    fn lines(from: u64) -> Vec<u8> {
        (from..from + 30)
            .flat_map(|number| format!("\x1b[3{}m{number}\x1b[0m\r\n", number % 8).into_bytes())
            .collect()
    }

    #[test]
    fn a_rebuild_that_continues_the_count_passes() {
        let rebuilt = snapshot(&["1", "2", "3", "4", "5", ""], 2, (0, 3), b"");
        assert_eq!(continuity(&rebuilt, &lines(6)), Ok(()));
        // The cursor after the first digit of 12.
        let rebuilt = snapshot(&["10", "11", "1"], 0, (1, 2), b"");
        let after = [b"2\x1b[0m\r\n".as_slice(), &lines(13)].concat();
        assert_eq!(continuity(&rebuilt, &after), Ok(()));
        // A control sequence split by the rebuild.
        let rebuilt = snapshot(&["10", "11", "12"], 0, (2, 2), b"\x1b[0");
        let after = [b"m\r\n".as_slice(), &lines(13)].concat();
        assert_eq!(continuity(&rebuilt, &after), Ok(()));
        // Between a line's carriage return and its line feed.
        let rebuilt = snapshot(&["4", "5"], 0, (0, 1), b"");
        assert_eq!(
            continuity(&rebuilt, &[b"\n".as_slice(), &lines(6)].concat()),
            Ok(())
        );
    }

    #[test]
    fn lost_or_repeated_output_fails() {
        let rebuilt = snapshot(&["1", "2", "3", "4", "5", ""], 0, (0, 5), b"");
        assert!(
            continuity(&rebuilt, &lines(7))
                .unwrap_err()
                .contains("lost")
        );
        assert!(
            continuity(&rebuilt, &lines(5))
                .unwrap_err()
                .contains("applied twice")
        );
        assert!(continuity(&rebuilt, b"6\r\n").unwrap_err().contains("only"));
    }

    #[test]
    fn strips_the_counts_control_sequences() {
        assert_eq!(strip_controls(b"\x1b[31m12\x1b[0m\n\x1b[3"), b"12\n");
    }
}
