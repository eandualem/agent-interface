#!/usr/bin/env python3
"""Check that the Interactions pane follows a window resize in a disposable
Xvfb desktop, with no input after the resize.

Run under xvfb-run + dbus-run-session as a normal user. Uses no live provider,
real home, credentials or tmux server: the one profile it serves has no
interactions. The captures land in target/linux-desktop-smoke/interactions/.
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
OUTPUT = ROOT / "target/linux-desktop-smoke/interactions"
OUTPUT.mkdir(parents=True, exist_ok=True)
# An earlier run's captures must not pass for this one's.
for earlier in OUTPUT.glob("*.png"):
    earlier.unlink()
PROFILE = "0a1b2c3d4e5f"


def wait_for(check, label, seconds=120):
    # One reading may take most of a minute on a constrained runner.
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        result = check()
        if result:
            return result
        time.sleep(0.3)
    raise AssertionError(f"Timed out: {label}")


# A Unix socket path is short: keep the profile's socket near the root.
with tempfile.TemporaryDirectory(prefix="interactions-smoke-", dir="/tmp") as directory:
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
    # One workspace whose only pane is Interactions, so no navigation is needed.
    library = root / "library"
    library.mkdir()
    leaf = dict(id="000000000003", agent=None, view="activity")
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

            def read():
                assert app.poll() is None, "application exited"
                command("import", "-window", window, str(OUTPUT / "current.png"))
                # Controls are sparse text, not a scanned page.
                tsv = command("tesseract", str(OUTPUT / "current.png"), "stdout", "--psm", "11", "tsv")
                (OUTPUT / "current.tsv").write_text(tsv)
                return [word for word in csv.DictReader(io.StringIO(tsv), delimiter="\t")
                        if (word.get("text") or "").strip()]

            def find(text):
                found = [word for word in read()
                         # A label can be read with a stray quote mark before it.
                         if word["text"].strip().lstrip("‘’'\"“”").rstrip(".,:;") == text]
                found.sort(key=lambda word: (int(word["top"]), int(word["left"])))
                return found[0] if found else None

            # The footer's note depends on the pane's width. A resize must
            # show the note for the new width with no further input: a redraw
            # asked for while the window draws gets no frame of its own.
            wait_for(lambda: find("Recorded"), "the Interactions footer")
            for size, note in (("2000", "causes"), ("1000", "communication"), ("2000", "causes")):
                command("xdotool", "windowsize", "--sync", window, size, "880")
                time.sleep(2)
                assert find(note), f"the footer did not follow a resize to {size} px"
                command("import", "-window", window, str(OUTPUT / f"footer-{size}.png"))
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
print(f"Interactions captures are in {OUTPUT}")
