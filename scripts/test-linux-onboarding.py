#!/usr/bin/env python3
"""Click through first run in a private Linux desktop: a fresh install makes
its first profile and starts its guide, all in the one workspace window.

Uses the real native app and a stand-in for the Rust Agent Backbone service
(scripts/tests/fixtures/fake_backbone.py), whose machine serves no profile
yet: first run sets it up with setup's `initialize`. No agent executable,
model, real provider, user home or shared tmux server is used. Run under xvfb-run +
dbus-run-session, as a normal user. At 100 % the opening scene plays and
settles into the first question; at 150 % it has already been seen.
"""
import csv
import io
import json
import os
import re
from pathlib import Path
import shutil
import sqlite3
import stat
import subprocess
import sys
import tempfile
import time
import tomllib

ROOT = Path(__file__).resolve().parents[1]
BINARY = Path(os.environ.get("ONBOARDING_TEST_BINARY", str(ROOT / "target/debug/agent-interface"))).resolve()
SCALE = int(os.environ.get("ONBOARDING_TEST_SCALE", "100"))
assert SCALE in (100, 150)
OUTPUT = ROOT / "target/linux-desktop-smoke/onboarding" / str(SCALE)
OUTPUT.mkdir(parents=True, exist_ok=True)
FIXTURE = ROOT / "scripts/tests/fixtures/fake_backbone.py"
TOKENS = tomllib.loads((ROOT / "crates/app/assets/design/tokens.toml").read_text())
PALETTE = TOKENS["color"]["dark"]
accent = tuple(bytes.fromhex(PALETTE["accent"][1:]))
surface = tuple(bytes.fromhex(PALETTE["surface"][1:]))
DISABLED_ACCENT = "#" + "".join(f"{round((a + s) / 2):02x}" for a, s in zip(accent, surface))
# The request bodies setup takes, as the service's fixtures give them.
SETUP_FIXTURES = ROOT / "crates/roster/src/profiles/setup/fixtures"
INITIALIZE = json.loads((SETUP_FIXTURES / "setup-initialize.json").read_text())
FIRST_AGENT = json.loads((SETUP_FIXTURES / "setup-first-agent.json").read_text())
REQUEST_ID = re.compile(r"[0-9a-f]{12}7[0-9a-f]{3}[89ab][0-9a-f]{15}")


def same(sent, expected):
    """Equal as JSON, types included: 1 is not true."""
    return json.dumps(sent, sort_keys=True) == json.dumps(expected, sort_keys=True)


def fresh_request_id(request_id):
    """A UUIDv7 in simple form, made within the contract's window: at most
    seven days before now, at most five minutes after."""
    if not REQUEST_ID.fullmatch(request_id):
        return False
    made = int(request_id[:12], 16) / 1000
    return time.time() - 7 * 24 * 3600 <= made <= time.time() + 5 * 60


def wait_for(check, description, timeout=40):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        result = check()
        if result:
            return result
        time.sleep(0.3)
    raise AssertionError(f"Timed out: {description}")


# Unix socket paths are limited to about 107 bytes, so the private root is short.
with tempfile.TemporaryDirectory(prefix="onboarding-", dir="/tmp") as directory:
    root = Path(directory)
    root.chmod(0o700)
    env = {k: v for k, v in os.environ.items()
           if k not in ("TMUX", "LD_PRELOAD")
           and not k.startswith(("BACKBONE_", "AGENT_INTERFACE_", "TMUX_WORKSPACES_"))}
    env.update(HOME=str(root), XDG_CONFIG_HOME=str(root / "config"),
               XDG_DATA_HOME=str(root / "data"), XDG_STATE_HOME=str(root / "state"),
               XDG_CACHE_HOME=str(root / "cache"), TMUX_TMPDIR=str(root / "tmux"),
               LIBGL_ALWAYS_SOFTWARE="1", FAKE_BACKBONE_ROOT=str(root))
    (root / "tmux").mkdir(mode=0o700)
    settings = root / "config/agent-interface"
    settings.mkdir(parents=True)
    seen = "true" if SCALE != 100 else "false"
    (settings / "settings.toml").write_text(
        f'theme = "dark"\ninterface_size = {SCALE}\n[setup]\nintro_seen = {seen}\ndone = false\n')
    # A fresh install: the workspace window makes the library folder as it opens.
    library = root / "data/agent-interface"
    bin_dir = root / ".local/bin"
    bin_dir.mkdir(parents=True)
    env["PATH"] = f"{bin_dir}:/usr/bin:/bin"

    def executable(name, body):
        path = bin_dir / name
        path.write_text(body)
        path.chmod(0o700)
        return path

    # The probe asks a login shell to find tools. Keep even that shell private.
    env["SHELL"] = str(executable("fixture-shell", '#!/bin/sh\nexec /bin/sh -c "$AGENT_INTERFACE_PROBE"\n'))
    real_tmux = shutil.which("tmux", path="/usr/bin:/bin")
    assert real_tmux
    # tmux ignores -L when -S is given, and every -S the stand-in hands out is
    # inside this root, so each call reaches a private server.
    executable("tmux", f'#!/bin/sh\nunset TMUX\nexec "{real_tmux}" -L onboarding-smoke "$@"\n')
    for name in ("claude", "codex"):
        executable(name, "#!/bin/sh\nexit 99\n")  # Found, but no real agent can run.
    executable("backbone", f'#!/bin/sh\nexec "{sys.executable}" -I "{FIXTURE}" "$@"\n')
    subprocess.run([str(bin_dir / "backbone"), "init"], env=env, check=True, capture_output=True)

    def command(*args, check=True, timeout=30):
        # OCR's OpenMP workers contend on constrained runners. Keep this limit
        # local to OCR; the app and window manager retain their environment.
        command_env = {**env, "OMP_THREAD_LIMIT": "1"} if args[0] == "tesseract" else env
        return subprocess.run(args, env=command_env, text=True, capture_output=True,
                              timeout=timeout, check=check).stdout.strip()

    def log(name):
        path = root / "fake" / name
        return [json.loads(line) for line in path.read_text().splitlines()] if path.exists() else []

    app = wm = server = None

    def app_windows():
        assert app.poll() is None, f"application exited with {app.returncode}"
        found = command("xdotool", "search", "--onlyvisible", "--pid", str(app.pid), check=False)
        return {window: command("xdotool", "getwindowname", window, check=False)
                for window in found.splitlines()}

    def words(window):
        assert app.poll() is None, f"application exited with {app.returncode}"
        shot = OUTPUT / "current.png"
        command("import", "-window", window, str(shot))
        tsv = command("tesseract", str(shot), "stdout", "--psm", "11", "tsv", timeout=60)
        found = [w for w in csv.DictReader(io.StringIO(tsv), delimiter="\t") if (w.get("text") or "").strip()]
        # Sparse-page OCR treats filled native buttons as illustrations. Locate
        # their painted fills, then read an inset crop as a single line.
        components = command("convert", str(shot), "-fuzz", "3%",
                             "-fill", "white", "-opaque", PALETTE["accent"],
                             "-opaque", DISABLED_ACCENT, "-fuzz", "0%", "-opaque", PALETTE["field"],
                             "-fill", "black", "+opaque", "white",
                             "-define", "connected-components:verbose=true", "-connected-components", "8", "null:")
        for index, line in enumerate(components.splitlines()):
            match = re.match(r"\s*\d+: (\d+)x(\d+)\+(\d+)\+(\d+) .*srgb\(255,255,255\)", line)
            if not match:
                continue
            width, height, x, y = map(int, match.groups())
            if width < 40 or height < 20 or height > 80 * SCALE / 100:
                continue
            inset = round(4 * SCALE / 100)
            crop = OUTPUT / "button.png"
            command("convert", str(shot), "-crop",
                    f"{width - 2 * inset}x{height - 2 * inset}+{x + inset}+{y + inset}",
                    "+repage", "-resize", "300%", str(crop))
            button_tsv = command("tesseract", str(crop), "stdout", "--psm", "7", "tsv", timeout=60)
            for word in csv.DictReader(io.StringIO(button_tsv), delimiter="\t"):
                if not (word.get("text") or "").strip():
                    continue
                for axis, origin in (("left", x + inset), ("top", y + inset)):
                    word[axis] = str(origin + int(word[axis]) // 3)
                for dimension in ("width", "height"):
                    word[dimension] = str(max(1, int(word[dimension]) // 3))
                found.append(word)
        return found

    def text(window):
        return " ".join(w["text"] for w in words(window))

    def click(window, label, last=False):
        def find():
            # Case matters: the footer's "start agent" hint is not "Start".
            found = [w for w in words(window) if w["text"].strip(".,:;‘’“”\"'…") == label]
            found.sort(key=lambda w: (int(w["top"]), int(w["left"])))
            return (found[-1] if last else found[0]) if found else None
        word = wait_for(find, label)
        command("xdotool", "mousemove", "--window", window,
                str(int(word["left"]) + int(word["width"]) // 2),
                str(int(word["top"]) + int(word["height"]) // 2))
        # Let the native window take the pointer's enter and motion first.
        time.sleep(0.2)
        command("xdotool", "click", "1")

    def capture(window, name):
        value = text(window)
        shutil.copyfile(OUTPUT / "current.png", OUTPUT / f"{name}.png")
        (OUTPUT / f"{name}.txt").write_text(value + "\n")
        return value

    with (OUTPUT / "app.log").open("w") as app_log, (OUTPUT / "wm.log").open("w") as wm_log, \
            (OUTPUT / "backbone.log").open("w") as server_log:
        try:
            server = subprocess.Popen([str(bin_dir / "backbone"), "serve"], env=env,
                                      stdout=server_log, stderr=subprocess.STDOUT)
            wait_for(lambda: (root / "fake/serve.pid").exists(), "the service")
            wm = subprocess.Popen(["xfwm4", "--compositor=on", "--sm-client-disable"],
                                  env=env, stdout=wm_log, stderr=subprocess.STDOUT)
            wait_for(lambda: "_GTK_FRAME_EXTENTS" in command("xprop", "-root", "_NET_SUPPORTED"), "window manager")
            app = subprocess.Popen([str(BINARY)], env=env, stdout=app_log, stderr=subprocess.STDOUT)
            main = wait_for(lambda: next(iter(app_windows()), None), "the app's window")
            command("xdotool", "mousemove", "--window", main, "5", "5")

            # This computer has everything, so nothing is asked of it: setup's
            # initialize makes its first profile while the scene (or, once seen,
            # first run itself) goes straight to naming the first profile.
            wait_for(lambda: re.search(r"first\s+profile", text(main)), "Name your first profile", timeout=60)
            setups = [entry["request"] for entry in log("setup.jsonl")]
            assert [r["action"] for r in setups] == ["initialize"], setups
            # The whole body, field for field as the service's fixture, but
            # its own request ID.
            assert setups[0].keys() == INITIALIZE.keys(), setups
            assert fresh_request_id(setups[0]["request_id"]), setups
            assert same({**setups[0], "request_id": INITIALIZE["request_id"]}, INITIALIZE), setups
            value = capture(main, "profile")
            assert "Agent Backbone" not in value, "the service was named on screen"
            assert "Personal" in value, "the suggested name"

            click(main, "Continue")
            wait_for(lambda: re.search(r"your\s+guide", text(main)), "Start your guide", timeout=60)
            # Each tool's card says whether it is signed in; nothing else opens.
            # OCR reads the plain cards reliably; the selected one is tinted.
            # Once both tools' account reads have answered, both cards say so.
            def account_reads():
                return {entry["path"].split("runtime=")[1] for entry in log("http.jsonl")
                        if entry["path"].startswith("/api/profile/accounts/status")}
            wait_for(lambda: {"claude", "codex"} <= account_reads(), "both tools' account reads", timeout=60)
            wait_for(lambda: len(re.findall(r"Sign\s+in\s+when\s+it\s+starts", text(main))) == 2,
                     "the tools' sign-ins on both cards", timeout=60)
            assert list(app_windows()) == [main], "first run opened or closed a window"
            # The service makes the profile and serves it at once: one create,
            # nothing to start.
            created = log("create.jsonl")
            assert len(created) == 1, created
            assert created[0]["entry"]["display_name"] == "Personal", created
            profile_id = created[0]["entry"]["profile_id"]
            commands = [entry["argv"][:2] for entry in log("cli.jsonl")]
            assert ["profiles", "apply"] not in commands, commands
            # Personal is the machine's second profile, so it keeps its own
            # accounts, which hold no login yet: both tools ask for a sign-in
            # when they start.
            assert created[0]["entry"]["accounts"] == "own", created
            accounts = {entry["path"].split("runtime=")[1]: entry.get("said") for entry in log("http.jsonl")
                        if entry["profile"] == profile_id and entry["path"].startswith("/api/profile/accounts/status")}
            assert accounts == {"claude": "missing", "codex": "missing"}, accounts
            assert stat.S_IMODE(library.stat().st_mode) == 0o700, "the library folder is private"
            capture(main, "guide")

            click(main, "Start", last=True)
            # The layer fades as the guide's pane opens.
            wait_for(lambda: not re.search(r"your\s+guide", text(main)), "the guide's pane", timeout=60)
            capture(main, "working")
            assert list(app_windows()) == [main], "first run opened or closed a window"

            setups = [entry["request"] for entry in log("setup.jsonl")]
            assert [r["action"] for r in setups] == ["initialize", "first_agent"], setups
            request = setups[1]
            # The whole body: the fixture's fields, with this run's choices.
            assert request.keys() == FIRST_AGENT.keys(), request
            assert request["agent"].keys() == FIRST_AGENT["agent"].keys(), request
            assert fresh_request_id(request["request_id"]), request
            assert request["request_id"] != setups[0]["request_id"], request
            assert same({**request, "request_id": None}, {
                **FIRST_AGENT, "request_id": None, "profile_id": profile_id, "start_greeting": True,
                "agent": {**FIRST_AGENT["agent"], "directory": str(root / "Agents/guide"),
                          "runtime": "claude", "model": None, "create_directory": True,
                          "name": "guide", "brief": {"template": "orchestrator-v1"}}}), request
            assert (root / "Agents/guide").is_dir()

            saved = tomllib.loads((settings / "settings.toml").read_text())
            assert saved["setup"] == {"intro_seen": True, "done": True}, saved["setup"]
            with sqlite3.connect(f"file:{library / 'layouts.db'}?mode=ro", uri=True) as db:
                layout = json.loads(db.execute("SELECT value FROM terminal_layout WHERE id = 1").fetchone()[0])
            spaces = layout["workspaces"]
            # The first profile takes over the starting workspace; no empty one is left.
            assert len(spaces) == 1, spaces
            assert spaces[0]["name"] == "Personal" and spaces[0]["profile_id"] == profile_id, spaces

            (OUTPUT / "requests.json").write_text(json.dumps(dict(created=created, setup=setups), indent=2) + "\n")
            (OUTPUT / "result.json").write_text(json.dumps(dict(scale=SCALE, passed=True, checks=[
                "nothing asked of a ready computer", "setup's initialize on a fresh machine",
                "first profile made and served by the service",
                "private library folder", "one window throughout",
                "sign-ins on the tool cards, from the profile's own accounts",
                "guide on the first tool that can run",
                "guide's setup request", "setup remembered as done",
                "one workspace bound to the profile"]), indent=2) + "\n")
            print(f"Linux onboarding regression passed at {SCALE}%")
        except BaseException:
            try:
                command("import", "-window", "root", str(OUTPUT / "failure.png"))
                shutil.copytree(root / "fake", OUTPUT / "backbone", dirs_exist_ok=True)
            except Exception as error:
                print(f"Could not capture the failure: {error}", file=sys.stderr)
            raise
        finally:
            if app is not None and app.poll() is None:
                app.terminate()
                app.wait(timeout=15)
            if wm is not None:
                wm.terminate()
                wm.wait(timeout=15)
            if server is not None:
                server.terminate()
                server.wait(timeout=15)
            # The wrapper only reaches this run's private server.
            command(str(bin_dir / "tmux"), "kill-server", check=False)
