#!/usr/bin/env python3
"""Capture the agents tree in the panel and the new pane, and a connection.

A stand-in profile serves agents on three machines: this one, another online
and another offline. One agent has two running reviews, a browser and two
swarms. The check captures the panel's tree folded and open, then clicks a
connection row and captures Connections opened beside the focused pane. Last,
in a tab whose pane is empty, it opens that agent in the new pane with the
right arrow, moves to a swarm member and attaches it with Enter.

Run under xvfb-run + dbus-run-session as a normal user. Uses no live provider,
real home, credentials or tmux server. The captures land in
target/linux-desktop-smoke/agents/, for a reader to look at.
"""
import csv
import http.server
import io
import json
import os
from pathlib import Path
import shlex
import socketserver
import sqlite3
import subprocess
import tempfile
import threading
import time
import tomllib

ROOT = Path(__file__).resolve().parents[1]
OUTPUT = ROOT / "target/linux-desktop-smoke/agents"
OUTPUT.mkdir(parents=True, exist_ok=True)
# An earlier run's captures must not pass for this one's.
for earlier in OUTPUT.glob("*.png"):
    earlier.unlink()
PROFILE = "0a1b2c3d4e5f"
HERE = "5f2c9e0d1b7a4e38"
STUDIO = "9a8b7c6d5e4f3a2b"
BUILD_BOX = "1c2d3e4f5a6b7c8d"
PEERS = ROOT / "crates/roster/src/profiles/peers/fixtures"


TOKENS = tomllib.loads((ROOT / "crates/app/assets/design/tokens.toml").read_text())


def token(group, name):
    # A size from the design tokens, so the layout below follows them.
    return TOKENS[group][name]


def wait_for(check, label, seconds=120):
    # One reading may take most of a minute on a constrained runner.
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        result = check()
        if result:
            return result
        time.sleep(0.3)
    raise AssertionError(f"Timed out: {label}")


def agent(name, state, machine=HERE):
    return dict(name=name, session=name, state=state,
                machine=dict(id=machine, name={HERE: "laptop", STUDIO: "studio", BUILD_BOX: "build-box"}[machine]))


AGENTS = [
    # The service lists runs only while some agent reports background work.
    dict(agent("lead", "busy"), background=dict(running=2, queued=0, processes=0)),
    agent("detach-coordinator", "busy"),
    agent("detach-coder", "idle", STUDIO),
    agent("gate-coder", "idle"),
    agent("gate-reviewer", "idle"),
    agent("helper", "busy"),
    agent("quiet", "idle"),
    # A name long enough that its counts give way: whole, never cut.
    agent("a-rather-long-agent-name", "idle"),
    agent("wide-coder", "busy"),
    agent("wide-reviewer", "idle"),
    agent("tester", "idle", STUDIO),
    agent("away", "offline", BUILD_BOX),
]
SWARMS = [
    dict(name="detach", status="active", initiator="lead", coordinator="detach-coordinator",
         members=["detach-coordinator", "detach-coder"]),
    dict(name="gate", status="active", initiator="lead", coordinator="gate-coder",
         members=["gate-coder", "gate-reviewer"]),
    dict(name="wide", status="active", initiator="a-rather-long-agent-name", coordinator="wide-coder",
         members=["wide-coder", "wide-reviewer"]),
]
MACHINES = [
    dict(id=HERE, name="laptop", home=True, state="online"),
    dict(id=STUDIO, name="studio", home=False, state="online"),
    dict(id=BUILD_BOX, name="build-box", home=False, state="offline", last_seen="2026-11-27T08:00:00Z"),
]


def review(number, model):
    return dict(run_id=f"0193a1b2c3d47e5f8a9b0c1d2e3f4a6{number}", agent_id=None, agent="lead",
                kind="review", label=f"Review round {number}", state="running", waiting_on=None,
                queued_at="2026-11-27T08:00:00Z", started_at="2026-11-27T08:01:00Z",
                finished_at=None, exit_code=None, session=None, progress=None,
                review=dict(runtime="claude", model=model, effort="high", base="develop", head="4f1c2b9"))


RUNS = [review(1, "gemini-4-neon"), review(2, "claude-sonnet-5-5")]


# A Unix socket path is short: keep the profile's socket near the root.
with tempfile.TemporaryDirectory(prefix="agents-smoke-", dir="/tmp") as directory:
    root = Path(directory)
    env = {k: v for k, v in os.environ.items()
           if k != "TMUX" and not k.startswith(("BACKBONE_", "AGENT_INTERFACE_", "AGENT_BROWSER_"))}
    # The app's interactive login shell reads a file ENV or BASH_ENV names
    # after .profile, which could put another `backbone` first again.
    env.pop("ENV", None)
    env.pop("BASH_ENV", None)
    env.update(HOME=str(root), XDG_CONFIG_HOME=str(root / "config"),
               XDG_DATA_HOME=str(root / "data"), XDG_STATE_HOME=str(root / "state"),
               XDG_CACHE_HOME=str(root / "cache"),
               TMUX_TMPDIR=str(root / "tmux"), LIBGL_ALWAYS_SOFTWARE="1")
    (root / "tmux").mkdir(mode=0o700)
    settings = root / "config/agent-interface"
    settings.mkdir(parents=True)
    (settings / "settings.toml").write_text("[setup]\nintro_seen = true\ndone = true\n")
    # lead's browser: an agent-browser session named as it is, whose daemon
    # is this script, on a page at github.com.
    browsers = root / "agent-browser"
    browsers.mkdir()
    (browsers / "lead.pid").write_text(str(os.getpid()))
    (browsers / "lead.target").write_text(json.dumps(dict(targetId="FIXTURE", url="https://github.com/example/repo")))
    (browsers / "a-rather-long-agent-name.pid").write_text(str(os.getpid()))
    (browsers / "a-rather-long-agent-name.target").write_text(json.dumps(dict(targetId="WIDE", url="https://example.com/")))
    env["AGENT_BROWSER_SOCKET_DIR"] = str(browsers)
    # A stand-in for the service: a `backbone` command that lists one served
    # profile, whose private socket below answers the reads.
    profile = root / "backbone/profiles" / PROFILE
    profile.mkdir(mode=0o700, parents=True)
    served = dict(protocol_version=1, profile_id=PROFILE,
                  instance_id="0190c3a2c1107a3b8c9d0e1f2a3b4c5d", machine=dict(id=HERE))
    entry = dict(served, state="serving", display_name="Work", home=True,
                 socket=str(profile / "api.sock"), tmux_socket=str(root / "tmux/profile.sock"),
                 credential_reference=dict(type="socket"))
    commands = root / ".local/bin"
    commands.mkdir(parents=True)
    listed = shlex.quote(json.dumps(dict(profiles=[entry])))
    shown = shlex.quote(json.dumps(entry))
    (commands / "backbone").write_text(
        '#!/bin/sh\n'
        'if [ "$#" -eq 3 ] && [ "$1" = profiles ] && [ "$2" = list ] && [ "$3" = --json ]; then\n'
        f"    printf '%s' {listed}; exit 0\n"
        'fi\n'
        'if [ "$#" -eq 5 ] && [ "$1" = profiles ] && [ "$2" = show ] && [ "$3" = --json ] '
        f'&& [ "$4" = -- ] && [ "$5" = {PROFILE} ]; then\n'
        f"    printf '%s' {shown}; exit 0\n"
        'fi\n'
        'exit 2\n')
    (commands / "backbone").chmod(0o755)
    env.update(SHELL="/bin/sh", PATH=f"{commands}:{env.get('PATH', '')}")
    (root / ".profile").write_text(f'PATH={shlex.quote(str(commands))}:"$PATH"\nexport PATH\n')
    overview = json.loads((PEERS / "peer-overview.json").read_text())

    stopping = threading.Event()

    class Provider(http.server.BaseHTTPRequestHandler):
        def do_GET(self):
            path = self.path.split("?", 1)[0]
            status = 200
            if path == "/api/stream" and "peer.connections" in self.path:
                # Connections reads its records only while this stream is
                # open: say hello, then keep it alive until the check ends.
                self.send_response(200)
                self.send_header("Content-Type", "text/event-stream")
                self.end_headers()
                try:
                    self.wfile.write(b'event: hello\ndata: {"cursor": "0"}\n\n')
                    self.wfile.flush()
                    while not stopping.wait(10):
                        self.wfile.write(b": alive\n\n")
                        self.wfile.flush()
                except OSError:
                    pass
                return
            if path == "/health":
                # The version check the app makes before each connection.
                body = dict(status="ok", version="0.1.0")
            elif path == "/api/profile":
                body = served
            elif path == "/api/agents":
                body = dict(items=AGENTS)
            elif path == "/api/swarms":
                body = dict(items=SWARMS)
            elif path == "/api/status/services":
                body = dict(machines=MACHINES)
            elif path == "/api/runs":
                body = dict(items=RUNS)
            elif path == "/api/peer-connections":
                body = overview
            elif path == "/api/peer-connections/reviews":
                body = json.loads((PEERS / "peer-reviews.json").read_text())
            elif path.startswith("/api/peer-connections/") and path.endswith("/history"):
                body = json.loads((PEERS / "peer-history.json").read_text())
            elif path.startswith(("/api/stream", "/api/peer-connections", "/api/conversations")):
                status, body = 404, {}
            else:
                body = dict(items=[], has_more=False, next_cursor=None)
            encoded = json.dumps(body).encode()
            self.send_response(status)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(encoded)))
            self.end_headers()
            self.wfile.write(encoded)

        def log_message(self, *_):
            pass

    class Server(socketserver.ThreadingUnixStreamServer):
        daemon_threads = True

    server = Server(str(profile / "api.sock"), Provider)
    os.chmod(profile / "api.sock", 0o600)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    # One workspace whose only pane is the Tags view: no shell starts, and
    # Connections has to open beside it.
    library = root / "library"
    library.mkdir()
    leaf = dict(id="000000000003", agent=None, view="tags")
    # A second tab whose only pane is empty: the new pane.
    chooser = dict(id="000000000005", agent=None, empty=True)
    model = dict(version=2, focus=False, selected="000000000001", workspaces=[
        dict(id="000000000001", name="Fixture", profile_id=PROFILE, selected="000000000002", tabs=[
            dict(id="000000000002", name="Fixture tab", focus=leaf["id"], tree=leaf),
            dict(id="000000000004", name="Chooser", focus=chooser["id"], tree=chooser)])])
    with sqlite3.connect(library / "layouts.db") as db:
        db.execute("CREATE TABLE terminal_layout (id INTEGER PRIMARY KEY, value TEXT)")
        db.execute("INSERT INTO terminal_layout VALUES (1, ?)", (json.dumps(model),))

    def command(*args, check=True):
        # Bounded OCR worker count on constrained runners.
        reading = args[0] == "tesseract"
        command_env = {**env, "OMP_THREAD_LIMIT": "1"} if reading else env
        return subprocess.run(args, env=command_env, text=True, capture_output=True,
                              timeout=60 if reading else 15, check=check).stdout.strip()

    profile_tmux = ["tmux", "-f", "/dev/null", "-S", str(root / "tmux/profile.sock")]
    app = wm = window = None
    with (OUTPUT / "app.txt").open("w") as app_log, (OUTPUT / "wm.txt").open("w") as wm_log:
        try:
            # The new pane lists the agents with a session on the profile's
            # own, private tmux server, which the cleanup below ends.
            for name in ("lead", "detach-coordinator", "quiet"):
                subprocess.run([*profile_tmux, "new-session", "-d", "-s", name, "sleep 600"],
                               env=env, check=True, timeout=15)
            wm = subprocess.Popen(["xfwm4", "--compositor=on", "--sm-client-disable"],
                                  env=env, stdout=wm_log, stderr=subprocess.STDOUT)
            wait_for(lambda: "_GTK_FRAME_EXTENTS" in command("xprop", "-root", "_NET_SUPPORTED"), "window manager")
            app = subprocess.Popen([str(ROOT / "target/debug/agent-interface"), "--data-dir", str(library)],
                                   env=env, stdout=app_log, stderr=subprocess.STDOUT)
            window = wait_for(lambda: command("xdotool", "search", "--onlyvisible", "--pid", str(app.pid), check=False),
                              "app window").splitlines()[0]
            command("xdotool", "windowactivate", "--sync", window)
            command("xdotool", "windowsize", "--sync", window, "1280", "880")
            time.sleep(0.6)

            def read():
                assert app.poll() is None, "application exited"
                command("import", "-window", window, str(OUTPUT / "current.png"))
                tsv = command("tesseract", str(OUTPUT / "current.png"), "stdout", "--psm", "11", "tsv")
                (OUTPUT / "current.tsv").write_text(tsv)
                return [word for word in csv.DictReader(io.StringIO(tsv), delimiter="\t")
                        if (word.get("text") or "").strip()]

            def found(text):
                words = [word for word in read()
                         if word["text"].strip().lstrip("‘’'\"“”").rstrip(".,:;") == text]
                return sorted(words, key=lambda word: (int(word["top"]), int(word["left"])))

            def find(text):
                words = found(text)
                return words[0] if words else None

            def press(x, y):
                command("xdotool", "mousemove", "--window", window, str(x), str(y))
                time.sleep(0.2)
                command("xdotool", "click", "1")

            def click(text):
                word = wait_for(lambda: find(text), text)
                press(int(word["left"]) + int(word["width"]) // 2, int(word["top"]) + int(word["height"]) // 2)

            def capture(name):
                command("xdotool", "mousemove", "--window", window, "640", "5")
                time.sleep(0.5)
                command("import", "-window", window, str(OUTPUT / f"{name}.png"))

            # Folded: one list, no machine sections, the offline agent on
            # another machine last, nothing under lead shown.
            heading = wait_for(lambda: find("AGENTS"), "the agents section")
            wait_for(lambda: find("lead"), "lead's row")
            away = wait_for(lambda: find("away"), "the offline agent on another machine")
            quiet = wait_for(lambda: find("quiet"), "an idle agent here")
            assert int(away["top"]) > int(quiet["top"]), "the offline remote agent is not sorted last"
            assert not find("online"), "a machine section header is still drawn"
            assert not find("gemini-4-neon") and not find("coordinator"), "a folded agent shows its children"
            capture("tree-folded")

            # Only the chevron folds. It is the last thing on the row, so its
            # middle is the panel's right edge less the rows' margin, the
            # row's padding and half a button. The panel's left is found from
            # its AGENTS label, which follows the section's padding and arrow.
            panel_left = int(heading["left"]) - (token("space", "s10") + token("size", "icon")
                                                 + token("space", "s4"))
            panel_right = panel_left + token("size", "panel_width")
            chevron_x = round(panel_right - token("space", "s8") - token("space", "s10")
                              - token("size", "icon_button") / 2)

            def chevron(name):
                # The section grows upward as rows open under an agent, so
                # the row is read again before each press, from the panel's
                # words only.
                row = wait_for(lambda: next((word for word in found(name)
                                             if int(word["left"]) < panel_right), None), f"{name}'s row")
                return chevron_x, int(row["top"]) + int(row["height"]) // 2

            press(*chevron("lead"))
            # Reviews, the browser by its host, and members by short name.
            wait_for(lambda: find("gemini-4-neon") or find("high"), "lead's running reviews")
            wait_for(lambda: find("github.com"), "lead's browser by its host")
            wait_for(lambda: find("coordinator"), "a swarm member by its short name")
            assert not find("detach-coordinator"), "a member shows its whole name"
            # The list holds nine rows and scrolls; the first swarm is in view.
            wait_for(lambda: find("detach"), "the first swarm")
            capture("tree-open")
            press(*chevron("lead"))
            # Only lead's own fold hides all of these at once; folding the
            # detach swarm alone would hide only its members.
            under_lead = {"coordinator", "github.com", "detach", "gemini-4-neon", "high"}
            wait_for(lambda: not under_lead & {word["text"].strip().rstrip(".,:;") for word in read()},
                     "lead folded again")
            capture("tree-refolded")

            # A connection row opens Connections beside the Tags pane, in
            # this tab, on that peer.
            click("Example")
            # The peer's name in a pane, right of the panel, not its row there.
            wait_for(lambda: any(int(word["left"]) > int(heading["left"]) + 300
                                 for word in found("Example")), "Connections opened on the peer")
            assert find("Tags") or find("TAGS"), "the Tags pane was replaced"
            capture("connections-beside")

            # The new pane, right of the panel.
            click("Chooser")

            def in_pane(text):
                return next((word for word in found(text) if int(word["left"]) > panel_right), None)

            # Its agents are read once the profile's team is: until then its
            # sessions are listed as plain sessions.
            wait_for(lambda: find("BACKBONE"), "the new pane's agents")
            wait_for(lambda: in_pane("lead"), "lead's row in the new pane")
            wait_for(lambda: find("expand"), "the new pane's hint bar")
            assert find("collapse"), "the hint bar lacks collapse"
            wait_for(lambda: in_pane("CONNECTIONS"), "the new pane's connections section")
            # Its members are not top-level rows any more.
            assert not find("detach-coordinator"), "a swarm member is a top-level row"
            # Keys go to the filter field; it is empty, so the arrows fold.
            field = wait_for(lambda: in_pane("Filter"), "the new pane's filter")
            press(int(field["left"]) + int(field["width"]) // 2, int(field["top"]) + int(field["height"]) // 2)
            # lead works, so it is the first row; Up stops there, from any of
            # the pane's rows: four agents, the fixture's connections and
            # All connections.
            for _ in range(12):
                command("xdotool", "key", "Up")
                time.sleep(0.2)
            command("xdotool", "key", "Right")
            wait_for(lambda: in_pane("coordinator"), "a swarm member under lead by its short name")
            capture("new-pane-open")
            # Under lead: two reviews with no view here, which the arrows
            # skip, then its browser, the detach swarm and its coordinator.
            for _ in range(3):
                command("xdotool", "key", "Down")
                time.sleep(0.3)
            command("xdotool", "key", "Return")

            def attached():
                clients = command(*profile_tmux, "list-clients", "-F", "#{session_name}", check=False)
                return "detach-coordinator" in clients.split()

            wait_for(attached, "Enter attaching the swarm member here")
            capture("new-pane-attached")
            (OUTPUT / "result.json").write_text(json.dumps(dict(checks=[
                "one list across three machines", "offline remote agent last", "folded hides children",
                "chevron opens reviews, browser host and short member names", "chevron folds again",
                "connection row opens Connections beside the focused pane",
                "new pane: right arrow opens an agent to its swarm members",
                "new pane: Enter on a member attaches it"]), indent=2))
        finally:
            for process in (app, wm):
                if process and process.poll() is None:
                    process.terminate()
                    try:
                        process.wait(timeout=10)
                    except subprocess.TimeoutExpired:
                        process.kill()
                        process.wait()
            subprocess.run(["tmux", "-S", str(root / "tmux/profile.sock"), "kill-server"],
                           env=env, capture_output=True, check=False)
            stopping.set()
            server.shutdown()
            server.server_close()
print(f"Agents captures are in {OUTPUT}")
