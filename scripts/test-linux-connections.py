#!/usr/bin/env python3
"""Capture the Connections pane's sample data in a disposable Xvfb desktop.

It visits each state the pane's design shows: a long message cut and opened,
messages waiting for review in both directions, the revise note, a failed
delivery, an incoming request, the connect popover, the minimised peer list
and the narrow layout, and saves one capture of each.

Run under xvfb-run + dbus-run-session as a normal user. Uses no live provider,
real home, credentials or tmux server: the one profile it serves refuses the
connection reads, so the pane offers its preview. The captures land beside
the desktop check's evidence, in target/linux-desktop-smoke/connections/, for
a reader to look at.
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

ROOT = Path(__file__).resolve().parents[1]
OUTPUT = ROOT / "target/linux-desktop-smoke/connections"
OUTPUT.mkdir(parents=True, exist_ok=True)
# An earlier run's captures must not pass for this one's.
for earlier in OUTPUT.glob("*.png"):
    earlier.unlink()
PROFILE = "0a1b2c3d4e5f"


def wait_for(check, label):
    # One reading may take most of a minute on a constrained runner.
    deadline = time.monotonic() + 120
    while time.monotonic() < deadline:
        result = check()
        if result:
            return result
        time.sleep(0.3)
    raise AssertionError(f"Timed out: {label}")


# A Unix socket path is short: keep the profile's socket near the root.
with tempfile.TemporaryDirectory(prefix="connections-smoke-", dir="/tmp") as directory:
    root = Path(directory)
    env = {k: v for k, v in os.environ.items()
           if k != "TMUX" and not k.startswith(("BACKBONE_", "AGENT_INTERFACE_"))}
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
    # A stand-in for the service: a `backbone` command that lists one served
    # profile, whose private socket below answers the reads.
    profile = root / "backbone/profiles" / PROFILE
    profile.mkdir(mode=0o700, parents=True)
    served = dict(protocol_version=1, profile_id=PROFILE,
                  instance_id="0190c3a2c1107a3b8c9d0e1f2a3b4c5d", machine=dict(id="5f2c9e0d1b7a4e38"))
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

    class Provider(http.server.BaseHTTPRequestHandler):
        def do_GET(self):
            path = self.path.split("?", 1)[0]
            status = 200
            if path == "/health":
                # The version check the app makes before each connection.
                body = dict(status="ok", version="0.1.0")
            elif path == "/api/profile":
                body = served
            elif path.startswith(("/api/peer-connections", "/api/conversations")):
                # With no connections to read, the pane offers its preview.
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
    # One workspace whose only pane is Connections, so no navigation is needed.
    library = root / "library"
    library.mkdir()
    leaf = dict(id="000000000003", agent=None, view="connections")
    model = dict(version=2, focus=False, selected="000000000001", workspaces=[
        dict(id="000000000001", name="Fixture", profile_id=PROFILE, selected="000000000002", tabs=[
            dict(id="000000000002", name="Fixture tab", focus=leaf["id"], tree=leaf)])])
    with sqlite3.connect(library / "layouts.db") as db:
        db.execute("CREATE TABLE terminal_layout (id INTEGER PRIMARY KEY, value TEXT)")
        db.execute("INSERT INTO terminal_layout VALUES (1, ?)", (json.dumps(model),))

    def command(*args, check=True):
        # Match the other checks' bounded OCR worker count on constrained
        # runners, where one reading can also take long.
        reading = args[0] == "tesseract"
        command_env = {**env, "OMP_THREAD_LIMIT": "1"} if reading else env
        return subprocess.run(args, env=command_env, text=True, capture_output=True,
                              timeout=60 if reading else 15, check=check).stdout.strip()

    app = wm = window = None
    with (OUTPUT / "app.txt").open("w") as app_log, (OUTPUT / "wm.txt").open("w") as wm_log:
        try:
            wm = subprocess.Popen(["xfwm4", "--compositor=on", "--sm-client-disable"],
                                  env=env, stdout=wm_log, stderr=subprocess.STDOUT)
            wait_for(lambda: "_GTK_FRAME_EXTENTS" in command("xprop", "-root", "_NET_SUPPORTED"), "window manager")
            app = subprocess.Popen([str(ROOT / "target/debug/agent-interface"), "--data-dir", str(library)],
                                   env=env, stdout=app_log, stderr=subprocess.STDOUT)
            window = wait_for(lambda: command("xdotool", "search", "--onlyvisible", "--pid", str(app.pid), check=False),
                              "app window").splitlines()[0]
            command("xdotool", "windowactivate", "--sync", window)
            command("xdotool", "windowsize", "--sync", window, "1280", "880")
            # The size is set before the window has drawn at it.
            time.sleep(0.6)

            def find(text):
                assert app.poll() is None, "application exited"
                command("import", "-window", window, str(OUTPUT / "current.png"))
                # Controls are sparse text, not a scanned page.
                tsv = command("tesseract", str(OUTPUT / "current.png"), "stdout", "--psm", "11", "tsv")
                (OUTPUT / "current.tsv").write_text(tsv)
                found = [word for word in csv.DictReader(io.StringIO(tsv), delimiter="\t")
                         if (word.get("text") or "").strip().rstrip(".,:;") == text]
                found.sort(key=lambda word: (int(word["top"]), int(word["left"])))
                return found[0] if found else None

            def click(text):
                word = wait_for(lambda: find(text), text)
                x = int(word["left"]) + int(word["width"]) // 2
                y = int(word["top"]) + int(word["height"]) // 2
                # The window has to see the pointer arrive before the press.
                command("xdotool", "mousemove", "--window", window, str(x), str(y))
                time.sleep(0.2)
                command("xdotool", "click", "1")

            def capture(name):
                # Move the pointer off the pane, so no row is drawn hovered.
                command("xdotool", "mousemove", "--window", window, "5", "5")
                time.sleep(0.5)
                command("import", "-window", window, str(OUTPUT / f"{name}.png"))

            def press(x, y):
                command("xdotool", "mousemove", "--window", window, str(x), str(y))
                time.sleep(0.2)
                command("xdotool", "click", "1")

            def scroll_thread():
                # A thread opens at its start; its waiting messages are last.
                command("xdotool", "mousemove", "--window", window, "800", "450")
                time.sleep(0.2)
                command("xdotool", "click", "--repeat", "15", "--delay", "30", "5")
                time.sleep(0.5)

            # Filled buttons are not read, so each wait is on plain text near
            # what it checks, one reading at a time.
            click("Preview")
            # The first sample connection opens with the list beside it.
            wait_for(lambda: find("PEERS"), "the peer list")
            wait_for(lambda: find("Relay"), "the first sample thread")
            capture("message-cut")
            # The first long message is cut; open it in place, then close it.
            click("Show")
            wait_for(lambda: find("less"), "the opened message")
            capture("message-opened")
            click("less")
            wait_for(lambda: find("Show"), "the message cut again")
            # The outgoing message waiting for review offers Ask to revise.
            scroll_thread()
            wait_for(lambda: find("Nothing"), "the waiting outgoing message")
            capture("waiting-outgoing")
            click("Ask")
            wait_for(lambda: find("should"), "the revise note")
            capture("revise")
            # Its Cancel is below the fold; Escape in the note closes it.
            command("xdotool", "key", "Escape")
            wait_for(lambda: find("Ask"), "the revise note closed")
            # The second connection has an incoming message waiting and a
            # failed delivery.
            click("Ops")
            wait_for(lambda: find("mailbox"), "the failed delivery")
            capture("failed")
            scroll_thread()
            wait_for(lambda: find("delivers"), "the waiting incoming message")
            capture("waiting-incoming")
            # The first unnamed row is the incoming request.
            click("Unnamed")
            wait_for(lambda: find("Declining"), "the incoming request")
            capture("request")
            # ＋ and the minimise button are icons, placed from the list's
            # PEERS label: the list is 248 px, its buttons 24 px at its right.
            label = wait_for(lambda: find("PEERS"), "the peer list")
            left = int(label["left"]) - 16
            middle = int(label["top"]) + int(label["height"]) // 2
            press(left + 202, middle)
            wait_for(lambda: find("Their"), "the connect popover")
            command("xdotool", "type", "--delay", "20", "v1:" + "7c" * 32)
            wait_for(lambda: find("Valid"), "the pasted ID checked")
            capture("connect")
            command("xdotool", "key", "Escape")
            wait_for(lambda: not find("Their"), "the connect popover closed")
            # Choose a connection while its name can still be read.
            click("Ops")
            wait_for(lambda: find("mailbox"), "the second thread again")
            press(left + 228, middle)
            wait_for(lambda: not find("PEERS"), "the minimised peer list")
            capture("minimised")
            # Narrower than 560 px of pane, a chooser in the header stands in
            # for the list, and the footer keeps only the waiting count.
            command("xdotool", "windowsize", "--sync", window, "700", "880")
            time.sleep(0.6)
            wait_for(lambda: not find("Agents"), "the narrow layout")
            capture("narrow")
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
            server.shutdown()
            server.server_close()
print(f"Connections captures are in {OUTPUT}")
