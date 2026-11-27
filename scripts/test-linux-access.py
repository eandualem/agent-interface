#!/usr/bin/env python3
"""Capture the Access pane against fixture data in a disposable Xvfb desktop.

It opens the agent list, an agent's detail, a refused change with
Backbone's reason, a granted repository read back from the stand-in, and the
requests view, and saves one capture of each.

Run under xvfb-run + dbus-run-session as a normal user. Uses no live provider,
real home, credentials or tmux server: the one profile it serves answers from
the roster crate's access fixtures, and keeps the changes it is sent in
memory. The captures land beside the desktop check's evidence, in
target/linux-desktop-smoke/access/, for a reader to look at.
"""
import copy
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
FIXTURES = ROOT / "crates/roster/src/profiles/access/fixtures"
OUTPUT = ROOT / "target/linux-desktop-smoke/access"
OUTPUT.mkdir(parents=True, exist_ok=True)
# An earlier run's captures must not pass for this one's.
for earlier in OUTPUT.glob("*.png"):
    earlier.unlink()
PROFILE = "0a1b2c3d4e5f"


def fixture(name):
    return json.loads((FIXTURES / f"{name}.json").read_text())


def wait_for(check, label, seconds=120):
    # One reading may take most of a minute on a constrained runner.
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        result = check()
        if result:
            return result
        time.sleep(0.3)
    raise AssertionError(f"Timed out: {label}")


class Records:
    """The stand-in's access records, changed by the requests it is sent."""

    def __init__(self):
        self.lock = threading.Lock()
        self.agents = fixture("agents")
        self.calls = []

    def agent(self, name):
        return next(a for a in self.agents["items"] if a.get("name") == name)

    def access(self, agent):
        reply = {k: agent[k] for k in ("name", "mode", "grants", "ceiling", "reach")}
        reply["warning"] = None
        return reply

    def change(self, method, path, body):
        """The status and body for a change, as Backbone would answer it."""
        with self.lock:
            self.calls.append((method, path, body))
            parts = path.strip("/").split("/")
            if parts[:2] == ["api", "agents"] and len(parts) == 4:
                name, action = parts[2], parts[3]
                if action == "mode":
                    # The owner-only change Backbone refuses in this check.
                    return 403, fixture("refusal-outside-authority")
                agent = self.agent(name)
                if action == "grant":
                    grant = body["grant"]
                    agent["grants"] = [g for g in agent["grants"] if not same(g, grant)] + [grant]
                    if grant["kind"] == "repository":
                        items = agent["reach"]["items"]
                        items[:] = [i for i in items if i["repo"] != grant["repo"]]
                        items.append(dict(repo=grant["repo"], access=grant["access"], source="grant"))
                    return 200, self.access(agent)
                if action == "revoke":
                    agent["grants"] = [g for g in agent["grants"] if not same(g, body["grant"])]
                    return 200, self.access(agent)
                if action == "place":
                    agent["role"], agent["superior"] = body["role"], body.get("superior")
                    return 200, dict(name=name, role=agent["role"], superior=agent["superior"])
            return 404, {"detail": {"code": "not_found", "message": "No such route in this check."}}


def same(a, b):
    """Whether two grants name the same thing, whatever their access."""
    keep = ("kind", "repo", "path", "name", "machine")
    return {k: a.get(k) for k in keep} == {k: b.get(k) for k in keep}


RECORDS = Records()

# A Unix socket path is short: keep the profile's socket near the root.
with tempfile.TemporaryDirectory(prefix="access-smoke-", dir="/tmp") as directory:
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
    # profile, whose private socket below answers the reads and changes.
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
        def reply(self, status, body):
            encoded = json.dumps(body).encode()
            self.send_response(status)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(encoded)))
            self.end_headers()
            self.wfile.write(encoded)

        def do_GET(self):
            path, _, query = self.path.partition("?")
            if path == "/health":
                # The version check the app makes before each connection.
                return self.reply(200, dict(status="ok", version="0.1.0"))
            if path == "/api/profile":
                return self.reply(200, served)
            if path == "/api/agents":
                with RECORDS.lock:
                    return self.reply(200, copy.deepcopy(RECORDS.agents))
            if path == "/api/swarms":
                return self.reply(200, fixture("swarms"))
            if path == "/api/access/requests":
                pending = "state=pending" in query
                return self.reply(200, fixture("requests-pending" if pending else "requests-all"))
            return self.reply(200, dict(items=[], has_more=False, next_cursor=None))

        def change(self):
            length = int(self.headers.get("Content-Length") or 0)
            body = json.loads(self.rfile.read(length) or b"{}")
            self.reply(*RECORDS.change(self.command, self.path.split("?", 1)[0], body))

        do_POST = change
        do_PUT = change

        def log_message(self, *_):
            pass

    class Server(socketserver.ThreadingUnixStreamServer):
        daemon_threads = True

    server = Server(str(profile / "api.sock"), Provider)
    os.chmod(profile / "api.sock", 0o600)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    # One workspace whose only pane is Access, so no navigation is needed.
    library = root / "library"
    library.mkdir()
    leaf = dict(id="000000000003", agent=None, view="access")
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

            def found(text):
                words = [word for word in read()
                         # A label can be read with a stray quote mark before it.
                         if word["text"].strip().lstrip("‘’'\"“”").rstrip(".,:;·") == text]
                words.sort(key=lambda word: (int(word["top"]), int(word["left"])))
                return words

            def find(text):
                words = found(text)
                return words[0] if words else None

            def click_word(word):
                x = int(word["left"]) + int(word["width"]) // 2
                y = int(word["top"]) + int(word["height"]) // 2
                # The window has to see the pointer arrive before the press.
                command("xdotool", "mousemove", "--window", window, str(x), str(y))
                time.sleep(0.2)
                command("xdotool", "click", "1")

            def click(text):
                click_word(wait_for(lambda: find(text), text))

            def capture(name):
                # Move the pointer off the pane, so no row is drawn hovered.
                command("xdotool", "mousemove", "--window", window, "5", "5")
                time.sleep(0.5)
                command("import", "-window", window, str(OUTPUT / f"{name}.png"))

            # The list groups the fixture's agents by role, the first chosen.
            wait_for(lambda: find("ORCHESTRATORS"), "the agent list")
            wait_for(lambda: find("SWARMS"), "the swarms group")
            wait_for(lambda: find("CONFINEMENT"), "the first agent's detail")
            capture("agents")

            # An unsandboxed main agent with folders, secrets and Docker.
            click("docs-site")
            wait_for(lambda: find("SITE_API_KEY"), "docs-site's secrets")
            wait_for(lambda: find("FOLDERS"), "docs-site's folders")
            capture("agent-detail")

            # Backbone refuses the confinement change; its reason is shown.
            click("tester")
            wait_for(lambda: find("Reaches"), "tester's confinement line")
            # The detail's Unsandboxed segment is the lowest, below the
            # toolbar's filter chip of the same word.
            segment = wait_for(lambda: (found("Unsandboxed") or [None])[-1], "the Unsandboxed segment")
            click_word(segment)
            wait_for(lambda: find("confined"), "Backbone's refusal")
            capture("refused")
            assert any(method == "PUT" and path == "/api/agents/tester/mode"
                       for method, path, _ in RECORDS.calls), RECORDS.calls

            # A repository granted from the detail is read back from Backbone.
            click("Add")
            wait_for(lambda: find("owner/name"), "the add field")
            command("xdotool", "type", "--delay", "20", "example/extra")
            command("xdotool", "key", "Return")
            wait_for(lambda: find("example/extra"), "the granted repository read back")
            capture("granted")
            assert any(method == "POST" and path == "/api/agents/tester/grant"
                       and body["grant"] == dict(kind="repository", repo="example/extra", access="read")
                       for method, path, body in RECORDS.calls), RECORDS.calls

            # The waiting requests, and Escape back to the agents.
            click("requests")
            wait_for(lambda: find("WAITING"), "the requests view")
            wait_for(lambda: find("Approve"), "a request's Approve")
            capture("requests")
            command("xdotool", "key", "Escape")
            wait_for(lambda: find("ORCHESTRATORS"), "the agent list again after Escape")
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
print(f"Access captures are in {OUTPUT}")
