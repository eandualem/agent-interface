#!/usr/bin/env python3
"""Capture the Access pane against fixture data in a disposable Xvfb desktop.

It opens the agent list, an agent's detail, a refused change with
Backbone's reason, a granted repository read back from the stand-in, the
requests view, the browsers with a browser's detail, and a new browser made,
signed in to and saved, and saves one capture of each.

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
BROWSER_FIXTURES = ROOT / "crates/roster/src/profiles/browsers/fixtures"
OUTPUT = ROOT / "target/linux-desktop-smoke/access"
OUTPUT.mkdir(parents=True, exist_ok=True)
# An earlier run's captures must not pass for this one's.
for earlier in OUTPUT.glob("*.png"):
    earlier.unlink()
PROFILE = "0a1b2c3d4e5f"


def fixture(name):
    return json.loads((FIXTURES / f"{name}.json").read_text())


def browser_fixture(name):
    return json.loads((BROWSER_FIXTURES / f"{name}.json").read_text())


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
        # Waiting requests; a decided one leaves them, as in Backbone.
        self.pending = fixture("requests-pending")
        self.browsers = browser_fixture("browsers")
        # The requests that wait for each browser made here, until it is saved.
        self.asking = {}
        # The site each browser made here is signed in to once it is checked.
        self.signing = {}
        self.calls = []

    def agent(self, name):
        return next(a for a in self.agents["items"] if a.get("name") == name)

    def access(self, agent):
        reply = {k: agent[k] for k in ("name", "mode", "grants", "ceiling", "reach")}
        reply["warning"] = None
        return reply

    def browser(self, id):
        return next((b for b in self.browsers["items"] if b["id"] == id), None)

    def detail(self, id):
        """A browser's detail: the fixture's, around the listed browser."""
        browser = self.browser(id)
        if browser is None:
            return 404, {"detail": {"code": "not_found", "message": f"No browser named '{id}'."}}
        detail = browser_fixture("browser")
        detail["browser"] = copy.deepcopy(browser)
        holder = browser.get("holder")
        detail["holder"] = holder and dict(holder, session=41, agent_browser_session=holder["agent"])
        # No Chrome runs in this check. A browser waiting for its sign-in is
        # served an endpoint, as Backbone serves one while its window is open,
        # so the app checks it; nothing connects to it.
        detail["devtools"] = "ws://127.0.0.1:9/devtools/browser/fixture" if id in self.signing else None
        detail["signed_in_to"] = [dict(domain=d, account=d, note=None) for d in browser["signed_in_to"]]
        if id != "youtube":
            detail["requests"] = self.asking.get(id, [])
            # The fixture's policies are YouTube's; another browser keeps to
            # its own sites.
            detail["policies"] = dict(keep_on_sites=True, sites=list(browser["signed_in_to"]),
                                      stop_before_last_button=False)
        return 200, detail

    def change_browser(self, method, parts, body):
        """A change of a browser, as Backbone would answer it."""
        if method == "POST" and len(parts) == 2:
            id = body["name"].lower()
            wanted = [w for w in self.browsers["wanted"] if w["browser"] == id]
            self.browsers["wanted"] = [w for w in self.browsers["wanted"] if w["browser"] != id]
            host = body["opens_at"].removeprefix("https://").strip("/")
            # As in Backbone, its sign-in is found only when it is checked; the
            # person's sign-in itself is not part of this check.
            self.signing[id] = host
            self.browsers["items"].append(dict(
                id=id, name=body["name"], machine="5f2c9e0d1b7a4e38", machine_name="laptop",
                opens_at=f"https://{host}/", signed_in_to=[], state="signing_in", agents=0,
                asks=sum(len(w["requests"]) for w in wanted), holder=None,
                signed_in_since="2026-11-27T10:40:00Z", checked_at="2026-11-27T10:40:00Z"))
            self.asking[id] = [r for w in wanted for r in w["requests"]]
            status, detail = self.detail(id)
            return (201 if status == 200 else status), detail
        browser = self.browser(parts[2])
        if browser is None:
            return self.detail(parts[2])
        action = parts[3] if len(parts) > 3 else None
        if method == "DELETE" and action == "hold":
            browser.update(holder=None, state="given" if browser["agents"] else "nobody")
            return 204, None
        if method == "DELETE" and action is None:
            self.browsers["items"].remove(browser)
            return 204, None
        if method == "POST" and action == "save":
            grants = body.get("grants", [])
            browser.update(name=body.get("name") or browser["name"], agents=len(grants), asks=0,
                           state="given" if grants else "nobody")
            self.asking.pop(browser["id"], None)
            return self.detail(browser["id"])
        if action == "check" and browser["id"] in self.signing:
            browser["signed_in_to"] = [self.signing.pop(browser["id"])]
        if action in ("window", "check", "policies"):
            return self.detail(browser["id"])
        return 404, {"detail": {"code": "not_found", "message": "No such route in this check."}}

    def change(self, method, path, body):
        """The status and body for a change, as Backbone would answer it."""
        with self.lock:
            self.calls.append((method, path, body))
            parts = path.strip("/").split("/")
            if parts[:2] == ["api", "browsers"]:
                return self.change_browser(method, parts, body)
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
            if parts[:3] == ["api", "access", "requests"] and len(parts) == 5:
                decided = fixture("request-approved")
                decided.update(id=int(parts[3]), note=body.get("note", ""),
                               state="approved" if parts[4] == "approve" else "refused")
                items = self.pending["items"]
                items[:] = [r for r in items if r["id"] != decided["id"]]
                return 200, decided
            return 404, {"detail": {"code": "not_found", "message": "No such route in this check."}}


def same(a, b):
    """Whether two grants name the same thing, whatever their access."""
    keep = ("kind", "repo", "path", "name", "machine", "browser")
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
            if status == 204:
                self.send_response(status)
                self.end_headers()
                return
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
            if path == "/api/browsers":
                with RECORDS.lock:
                    return self.reply(200, copy.deepcopy(RECORDS.browsers))
            if path.startswith("/api/browsers/") and path.count("/") == 3:
                with RECORDS.lock:
                    return self.reply(*copy.deepcopy(RECORDS.detail(path.rsplit("/", 1)[1])))
            if path == "/api/access/requests":
                if "state=pending" in query:
                    with RECORDS.lock:
                        return self.reply(200, copy.deepcopy(RECORDS.pending))
                return self.reply(200, fixture("requests-all"))
            return self.reply(200, dict(items=[], has_more=False, next_cursor=None))

        def change(self):
            length = int(self.headers.get("Content-Length") or 0)
            body = json.loads(self.rfile.read(length) or b"{}")
            self.reply(*RECORDS.change(self.command, self.path.split("?", 1)[0], body))

        do_POST = change
        do_PUT = change
        do_DELETE = change

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

            def containing(fragment):
                # Words holding `fragment`, top first: OCR joins or misreads
                # letters around a word ("Adda repository"), not inside it.
                words = [word for word in read() if fragment in word["text"]]
                words.sort(key=lambda word: (int(word["top"]), int(word["left"])))
                return words

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

            def click_agent(name):
                # An agent's row in the Access list, never the panel's agent
                # list beside it, which names the same agents: only words
                # right of the list's first heading count. The whole name must
                # show, so a row that cuts it short fails here.
                heading = wait_for(lambda: find("ORCHESTRATORS"), "the agent list")
                edge = int(heading["left"]) - 20
                click_word(wait_for(lambda: next(
                    (word for word in found(name) if int(word["left"]) >= edge), None),
                    f"{name}'s whole name in the Access list"))

            # An unsandboxed main agent with folders, secrets and Docker.
            click_agent("docs-site")
            # Only an unsandboxed agent's line says "everything".
            wait_for(lambda: find("everything"), "docs-site's confinement line")
            # Parts of its rows OCR keeps: the arm runner read SITE_API_KEY as
            # SITE_APL_KEY, and the folder /home/user/media whole.
            folder = wait_for(lambda: next(iter(containing("media")), None), "docs-site's folder")
            capture("agent-detail")

            def wheel(button, times):
                # Turn the wheel over the detail, where the folder line was
                # drawn: the detail scrolls under the pointer, not the list.
                command("xdotool", "mousemove", "--window", window,
                        str(int(folder["left"])), str(int(folder["top"])))
                time.sleep(0.2)
                command("xdotool", "click", "--repeat", str(times), "--delay", "50", button)
                time.sleep(0.3)

            # Its secrets and Docker are below the browser sign-ins, past the
            # bottom of the detail at this window's height.
            def secret_shown():
                wheel("5", 3)
                return containing("SITE_")
            wait_for(secret_shown, "docs-site's secret, scrolled to")
            # The detail's first heading, ROLE AND PLACE, is now above view.
            wait_for(lambda: not find("ROLE"), "docs-site's first heading scrolled out of view")

            # Another agent's detail opens at its top, not at docs-site's
            # scroll. Planner is not in docs-site's "answers to" line.
            click_agent("planner")
            # The header's role pill: planner is a coordinator, while builder,
            # whose row names planner, and docs-site are main agents. A row
            # shows no role.
            wait_for(lambda: find("Coordinator"), "planner's detail")
            wait_for(lambda: find("ROLE"), "planner's detail shown from its top")
            # Planner's own detail scrolls far enough to hide that heading, so
            # an offset carried over from docs-site would have hidden it too.
            def planner_heading_hidden():
                wheel("5", 3)
                return not find("ROLE")
            wait_for(planner_heading_hidden, "planner's first heading scrolled out of view")

            # Backbone refuses the confinement change; its reason is shown.
            click_agent("tester")
            # A sandboxed agent's line, "Reaches only what is listed below.";
            # docs-site's line also starts with "Reaches".
            wait_for(lambda: find("listed"), "tester's confinement line")
            # The detail's Unsandboxed segment is the lowest, below the
            # toolbar's filter chip of the same word.
            segment = wait_for(lambda: (found("Unsandboxed") or [None])[-1], "the Unsandboxed segment")
            click_word(segment)
            wait_for(lambda: find("confined"), "Backbone's refusal")
            capture("refused")
            assert any(method == "PUT" and path == "/api/agents/tester/mode"
                       for method, path, _ in RECORDS.calls), RECORDS.calls

            def sent(change):
                # Whether the stub has received a change matching `change`.
                return lambda: any(change(*call) for call in list(RECORDS.calls))

            # A repository granted from the detail is read back from Backbone.
            # GitHub's "Add a repository" is the first line starting "Add".
            add = wait_for(lambda: next((w for w in containing("Add")
                                         if w["text"].startswith("Add")), None),
                           "GitHub's Add a repository")
            click_word(add)
            # The open field's own Add button stands at the card's right end;
            # closed, "Add" only begins the line at its left.
            wait_for(lambda: any(word["text"] == "Add" and int(word["left"]) > int(add["left"]) + 300
                                 for word in read()), "the add field")
            command("xdotool", "type", "--delay", "20", "example/extra")
            command("xdotool", "key", "Return")
            wait_for(sent(
                lambda method, path, body: method == "POST" and path == "/api/agents/tester/grant"
                and body["grant"] == dict(kind="repository", repo="example/extra", access="read")),
                "the granted repository sent")

            def granted_shown():
                # Once the field has closed, so the typed text cannot match,
                # the new repository line is on screen.
                words = read()
                field_open = any(word["text"] == "Add" and int(word["left"]) > int(add["left"]) + 300
                                 for word in words)
                return not field_open and any("extra" in word["text"] for word in words)
            wait_for(granted_shown, "the granted repository shown")
            capture("granted")

            # The waiting requests, and Escape back to the agents.
            click_word(wait_for(lambda: next(iter(containing("request")), None),
                                "the requests waiting button"))
            wait_for(lambda: containing("WAIT"), "the requests view")
            wait_for(lambda: containing("Appro"), "a request's Approve")
            # The first card's reason, "Check the sign-up page as a visitor
            # sees it.", read here so its absence later means the card left.
            wait_for(lambda: containing("visit"), "the first card's reason")
            capture("requests")
            # The keys alone: choose the first card, open its note, type and
            # keep it with Return, then approve with the note. No click comes
            # between, so the keys reach the pane only if focus came back.
            command("xdotool", "key", "Down")
            # The footer says "note" already; the open field's hint adds one.
            notes = len(containing("note"))
            command("xdotool", "key", "n")
            wait_for(lambda: len(containing("note")) > notes, "the note field")
            command("xdotool", "type", "--delay", "20", "for this check")
            command("xdotool", "key", "Return")
            command("xdotool", "key", "a")
            wait_for(sent(
                lambda method, path, body: method == "POST" and path.endswith("/approve")
                and body.get("note") == "for this check"),
                "the approval sent with its note by the keys")
            # The approved card leaves Waiting, its note with it, when the
            # pane applies the reply or its next read no longer lists it.
            # Escape while the note is open would only close the note, and
            # the wait for the agent list below would then fail.
            wait_for(lambda: not containing("visit"), "the approved card gone from Waiting")
            command("xdotool", "key", "Escape")
            wait_for(lambda: find("ORCHESTRATORS"), "the agent list again after Escape")

            # The browsers, the first chosen: its holder, who may use it and
            # give it out, what it is signed in to and its recent use.
            click("Browsers")
            wait_for(lambda: find("BROWSERS"), "the browser list")
            wait_for(lambda: containing("WHO"), "the first browser's detail")
            wait_for(lambda: find("Take"), "its holder's Take it back")
            capture("browsers")
            click("Take")
            wait_for(sent(lambda method, path, _: method == "DELETE"
                          and path == "/api/browsers/instagram/hold"), "the browser taken back")
            wait_for(lambda: not find("Take"), "the holder gone after taking it back")
            capture("browser-detail")

            # A new browser for the one an agent asked for: named, opened,
            # signed in, the asking agent chosen, and saved.
            click("New")
            wait_for(lambda: find("Name"), "the new browser sheet")
            command("xdotool", "type", "--delay", "20", "Telegram")
            click_word(wait_for(lambda: next(iter(containing("site.com")), None), "the address field"))
            command("xdotool", "type", "--delay", "20", "web.telegram.org")
            command("xdotool", "key", "Return")
            wait_for(sent(lambda method, path, body: method == "POST" and path == "/api/browsers"
                          and body == dict(name="Telegram", opens_at="web.telegram.org")),
                     "the new browser created")
            # The pane checks the sign-in while the sheet waits for one.
            wait_for(sent(lambda method, path, _: method == "POST"
                          and path == "/api/browsers/telegram/check"), "the sign-in checked")
            # "Signed in to", which only a found sign-in shows.
            wait_for(lambda: find("Signed"), "the site found signed in")
            wait_for(lambda: find("Choose"), "who may use it")
            wait_for(lambda: containing("uploader"), "the agent that asked, offered")
            capture("new-browser")
            # Save browser's dark label on its accent fill is not read by OCR;
            # the button sits just left of Cancel on the same line, and the
            # request sent below is the proof it was pressed.
            cancel = wait_for(lambda: find("Cancel"), "the sheet's Cancel")
            command("xdotool", "mousemove", "--window", window,
                    str(int(cancel["left"]) - 40), str(int(cancel["top"]) + int(cancel["height"]) // 2))
            time.sleep(0.2)
            command("xdotool", "click", "1")
            wait_for(sent(lambda method, path, body: method == "POST"
                          and path == "/api/browsers/telegram/save"
                          and body["grants"] == ["uploader"] and body["name"] == "Telegram"),
                     "the new browser saved for the agent that asked")
            wait_for(lambda: containing("WHO"), "the saved browser's detail")
            capture("browser-saved")
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
