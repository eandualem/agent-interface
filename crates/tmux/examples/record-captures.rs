//! Records the `capture-pane` replies that the capture tests read, from
//! fixtures drawn on a private tmux server.
//!
//! Run it once for each tmux version, with that version's `tmux` first on
//! `PATH`:
//!
//! ```text
//! PATH=/path/to/tmux-3.3a:$PATH cargo run -p agent-interface-tmux --example record-captures
//! ```
//!
//! It writes `crates/tmux/tests/captures/VERSION/FIXTURE.txt`.

use std::path::Path;

use agent_interface_test_support::TmuxServer;

/// Fixtures drawn in a 20x5 pane: a name and the `printf` format that draws
/// it.
const ROWS: &[(&str, &str)] = &[
    (
        "styles",
        r"\033[1mB\033[22m\033[2mD\033[22m\033[3mI\033[23m\033[4mU\033[4:2mW\033[4:3mC\033[4:4mT\033[4:5mH\033[24m\033[5mK\033[25m\033[7mV\033[27m\033[8mX\033[28m\033[9mS\033[29m\033[53mO\033[55m\033[58;5;196;4mL\033[59;24m\033[58;2;1;2;3;4mR\033[0m\n\033[31ma\033[91mb\033[38;5;123mc\033[38;2;10;20;30md\033[39;44me\033[104mf\033[48;5;200mg\033[48;2;4;5;6mh\033[0m\n",
    ),
    ("carry", r"\033[1;32mgreen\nstill\033[0m plain\n\033[45m"),
    ("line-drawing", r"\033(0lqqk\nx  x\033(B ok\n"),
    (
        "links",
        r"\033]8;;http://a.example/\033\\one\033]8;;\033\\ two \033]8;id=k;http://b.example/\033\\0123456789012345678901234\033]8;;\033\\ end\n",
    ),
    (
        "wide",
        r"\347\225\214e\314\201\360\237\221\251\342\200\215\360\237\222\273\342\232\240\357\270\217x\n",
    ),
    ("tabs", r"\033[44m\033[2K\033[0ma\tb\033[41m\t\033[0mc\n"),
    // tmux doubles a backslash only in a cell of one byte, so one with a
    // combining mark is captured as it is.
    ("backslash-combining", r"\\\314\201\\x\n"),
    (
        "background-erase",
        r"a\033[44m\033[K\033[0m\nb\033[41m   \033[0m\n",
    ),
    ("text", r"%%begin 1 2 3\nback\\slash 100%%\n"),
    (
        "alternate",
        r"P1\nP2\n\033[1;31mP3\033[?1049h\033[HALT\033[0m",
    ),
];

/// Fixtures that leave the pane's parser inside a sequence.
const PENDING: &[(&str, &str)] = &[
    ("pending-csi", r"pre\033[3"),
    ("pending-csi-line-feed", r"top\n\033[1\n2"),
    ("pending-osc", r"pre\033]0;ti\ntle"),
    // An OSC interrupted by a control sequence: tmux keeps the bytes of both.
    ("pending-osc-csi", r"pre\033]0;ti\033[1\n2"),
];

fn main() -> std::io::Result<()> {
    let server = TmuxServer::start()?;
    let version = server.run(&["display-message", "-p", "#{version}"])?;
    let directory = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/captures")
        .join(version.trim());
    std::fs::create_dir_all(&directory)?;
    for &(name, format) in ROWS.iter().chain(PENDING) {
        // The pane signals once printf has finished, then stays open.
        let signal = server.shell_command(&["wait-for", "-S", name]);
        let program = format!("printf '{format}'; {signal}; exec sleep 1000");
        server.run(&[
            "new-session",
            "-d",
            "-s",
            name,
            "-x",
            "20",
            "-y",
            "5",
            &program,
        ])?;
        server.run(&["wait-for", name])?;
        let target = format!("{name}:");
        let pending = PENDING.iter().any(|&(fixture, _)| fixture == name);
        let capture = if pending {
            server.run(&["capture-pane", "-p", "-P", "-C", "-t", &target])?
        } else {
            server.run(&[
                "capture-pane",
                "-p",
                "-e",
                "-N",
                "-C",
                "-S",
                "-",
                "-E",
                "-",
                "-t",
                &target,
            ])?
        };
        std::fs::write(directory.join(format!("{name}.txt")), capture)?;
        if name == "alternate" {
            let saved = server.run(&[
                "capture-pane",
                "-p",
                "-e",
                "-N",
                "-C",
                "-a",
                "-q",
                "-S",
                "-",
                "-E",
                "-",
                "-t",
                &target,
            ])?;
            std::fs::write(directory.join("alternate-saved.txt"), saved)?;
        }
    }
    println!(
        "recorded tmux {} into {}",
        version.trim(),
        directory.display()
    );
    Ok(())
}
