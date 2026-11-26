#!/usr/bin/env python3
"""Capture the Connections pane's sample data in a disposable Xvfb desktop.

Run under xvfb-run + dbus-run-session as a normal user. Uses no live provider,
real home, credentials or tmux server: the one profile it lists is not served,
so the pane offers its preview. The captures land beside the desktop check's
evidence, in target/linux-desktop-smoke/connections/, for a reader to look at.
"""
import csv
import io
import json
import os
from pathlib import Path
import shlex
import sqlite3
import subprocess
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]
OUTPUT = ROOT / "target/linux-desktop-smoke/connections"
OUTPUT.mkdir(parents=True, exist_ok=True)
PROFILE = "0a1b2c3d4e5f"


def wait_for(check, label):
    deadline = time.monotonic() + 40
    while time.monotonic() < deadline:
        result = check()
        if result:
            return result
        time.sleep(0.3)
    raise AssertionError(f"Timed out: {label}")


with tempfile.TemporaryDirectory(prefix="connections-smoke-") as directory:
    root = Path(directory)
    env = {k: v for k, v in os.environ.items()
           if k != "TMUX" and not k.startswith(("BACKBONE_", "AGENT_INTERFACE_"))}
    # The app's interactive login shell reads a file ENV or BASH_ENV names
    # after .profile, which could put another `backbone` first again.
    env.pop("ENV", None)
    env.pop("BASH_ENV", None)
    env.update(HOME=str(root), XDG_CONFIG_HOME=str(root / "config"),
               XDG_DATA_HOME=str(root / "data"), XDG_STATE_HOME=str(root / "state"),
               TMUX_TMPDIR=str(root / "tmux"), LIBGL_ALWAYS_SOFTWARE="1")
    (root / "tmux").mkdir(mode=0o700)
    settings = root / "config/agent-interface"
    settings.mkdir(parents=True)
    (settings / "settings.toml").write_text("[setup]\nintro_seen = true\ndone = true\n")
    # A `backbone` command that lists one profile whose socket nobody serves.
    profile = root / "backbone/profiles" / PROFILE
    profile.mkdir(mode=0o700, parents=True)
    entry = dict(protocol_version=1, profile_id=PROFILE,
                 instance_id="0190c3a2c1107a3b8c9d0e1f2a3b4c5d", machine=dict(id="5f2c9e0d1b7a4e38"),
                 state="serving", display_name="Work", home=True,
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
        # Match the other checks' bounded OCR worker count on constrained runners.
        command_env = {**env, "OMP_THREAD_LIMIT": "1"} if args[0] == "tesseract" else env
        return subprocess.run(args, env=command_env, text=True, capture_output=True,
                              timeout=15, check=check).stdout.strip()

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
            command("xdotool", "windowsize", "--sync", window, "1280", "820")

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
                command("xdotool", "mousemove", "--window", window, str(x), str(y), "click", "1")

            def capture(name):
                # Move the pointer off the pane, so no row is drawn hovered.
                command("xdotool", "mousemove", "--window", window, "5", "5")
                time.sleep(0.5)
                command("import", "-window", window, str(OUTPUT / f"{name}.png"))

            click("Preview")
            wait_for(lambda: find("Sample"), "sample data")
            capture("preview")
            click("priya-platform")
            wait_for(lambda: find("Sample"), "sample data after choosing a peer")
            capture("peer")
        finally:
            for process in (app, wm):
                if process and process.poll() is None:
                    process.terminate()
                    try:
                        process.wait(timeout=10)
                    except subprocess.TimeoutExpired:
                        process.kill()
            subprocess.run(["tmux", "-S", str(root / "tmux/profile.sock"), "kill-server"],
                           env=env, capture_output=True, check=False)
print(f"Connections captures are in {OUTPUT}")
