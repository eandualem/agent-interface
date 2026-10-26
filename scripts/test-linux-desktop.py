#!/usr/bin/env python3
"""Exercise the real Linux window in a disposable Xvfb/Xfwm desktop.

Run with dbus-run-session and xvfb-run. All library, home and tmux state is
private to this test; no session from the invoking desktop is used.
"""
import json
import os
from pathlib import Path
import sqlite3
import subprocess
import tempfile
import time
import tomllib

ROOT = Path(__file__).resolve().parents[1]
OUTPUT = ROOT / "target/linux-desktop-smoke"
OUTPUT.mkdir(parents=True, exist_ok=True)


def wait_for(check, description):
    deadline = time.monotonic() + 20
    while time.monotonic() < deadline:
        result = check()
        if result:
            return result
        time.sleep(0.1)
    raise AssertionError(f"Timed out: {description}")


with tempfile.TemporaryDirectory(prefix="agent-interface-desktop-") as directory:
    root = Path(directory)
    env = {key: value for key, value in os.environ.items()
           if key != "TMUX" and not key.startswith(("BACKBONE_", "AGENT_INTERFACE_", "TMUX_WORKSPACES_"))}
    env.update(HOME=str(root), XDG_CONFIG_HOME=str(root / "config"),
               XDG_DATA_HOME=str(root / "data"), XDG_STATE_HOME=str(root / "state"),
               TMUX_TMPDIR=str(root / "tmux"), LIBGL_ALWAYS_SOFTWARE="1")
    (root / "tmux").mkdir(mode=0o700)
    library = root / "library"
    library.mkdir()

    def command(*args, check=True):
        return subprocess.run(args, env=env, text=True, stdout=subprocess.PIPE,
                              stderr=subprocess.PIPE, check=check).stdout.strip()

    wm_log = (OUTPUT / "window-manager.log").open("w")
    app_log = (OUTPUT / "app.log").open("w")
    wm = subprocess.Popen(["xfwm4", "--compositor=on", "--sm-client-disable"], env=env,
                          stdout=wm_log, stderr=subprocess.STDOUT)
    app = None
    server_created = False
    try:
        wait_for(lambda: "_GTK_FRAME_EXTENTS" in command("xprop", "-root", "_NET_SUPPORTED"), "window manager ready")
        command("tmux", "-L", "desktop-smoke", "-f", "/dev/null", "new-session", "-d",
                "-s", "fixture", "-x", "120", "-y", "40",
                "printf 'Agent Interface terminal spacing\n0123456789 ABCDEFGHIJ abcdefghij\n|||||||||| iiiiiiiiii MMMMMMMMMM\n'; exec /bin/sh")
        server_created = True
        socket = command("tmux", "-L", "desktop-smoke", "display-message", "-p", "#{socket_path}")
        leaf = {"id": "000000000003", "agent": "fixture", "source_socket": socket, "cwd": str(root)}
        model = {"version": 2, "focus": False, "selected": "000000000001", "workspaces": [
            {"id": "000000000001", "name": "Ubuntu test", "selected": "000000000002", "tabs": [
                {"id": "000000000002", "name": "Terminal", "focus": leaf["id"], "tree": leaf}]}]}
        with sqlite3.connect(library / "layouts.db") as connection:
            connection.execute("CREATE TABLE terminal_layout (id INTEGER PRIMARY KEY, value TEXT)")
            connection.execute("INSERT INTO terminal_layout VALUES (1, ?)", (json.dumps(model),))
        app = subprocess.Popen([str(ROOT / "target/debug/agent-interface"), "--data-dir", str(library)],
                               env=env, stdout=app_log, stderr=subprocess.STDOUT)
        window = wait_for(lambda: command("xdotool", "search", "--onlyvisible", "--pid", str(app.pid), check=False), "app window").splitlines()[0]
        command("xdotool", "windowactivate", "--sync", window)
        time.sleep(2)  # Allow the asynchronous library and terminal connection to paint.

        def geometry():
            fields = command("xdotool", "getwindowgeometry", "--shell", window)
            return {key: int(value) for key, value in (line.split("=", 1) for line in fields.splitlines())}

        def state():
            return command("xprop", "-id", window, "_NET_WM_STATE")

        caption = tomllib.loads((ROOT / "crates/app/assets/design/tokens.toml").read_text())["size"]["titlebar"]

        def click_button(position_from_right):
            width = geometry()["WIDTH"]
            command("xdotool", "mousemove", "--window", window,
                    str(round(width - caption * position_from_right)), str(round(caption / 2)), "click", "1")

        initial = geometry()
        command("import", "-window", window, str(OUTPUT / "window.png"))
        click_button(1.5)
        wait_for(lambda: "_NET_WM_STATE_MAXIMIZED_HORZ" in state() and "_NET_WM_STATE_MAXIMIZED_VERT" in state(), "maximize button")
        command("import", "-window", window, str(OUTPUT / "maximized.png"))
        click_button(1.5)
        wait_for(lambda: "MAXIMIZED" not in state() and geometry()["WIDTH"] == initial["WIDTH"], "restore button")
        command("xdotool", "mousemove", "--window", window, "120", str(round(caption / 2)),
                "click", "--repeat", "2", "--delay", "100", "1")
        wait_for(lambda: "_NET_WM_STATE_MAXIMIZED_HORZ" in state(), "caption double-click maximize")
        # GPUI groups successive clicks for 400 ms, even across maximization.
        time.sleep(0.6)
        command("xdotool", "mousemove", "--window", window, "120", str(round(caption / 2)),
                "click", "--repeat", "2", "--delay", "100", "1")
        wait_for(lambda: "MAXIMIZED" not in state(), "caption double-click restore")
        before_resize = geometry()
        command("xdotool", "mousemove", "--window", window,
                str(before_resize["WIDTH"] - 2), str(before_resize["HEIGHT"] - 2), "mousedown", "1")
        command("xdotool", "mousemove", "--window", window,
                str(before_resize["WIDTH"] + 50), str(before_resize["HEIGHT"] + 30), "mouseup", "1")
        wait_for(lambda: geometry()["WIDTH"] > before_resize["WIDTH"], "resize corner")
        click_button(2.5)
        wait_for(lambda: "_NET_WM_STATE_HIDDEN" in state(), "minimize button")
        command("xdotool", "windowactivate", "--sync", window)
        wait_for(lambda: "_NET_WM_STATE_HIDDEN" not in state(), "restore minimized window")
        click_button(0.5)
        assert app.wait(timeout=15) == 0, "close button should exit normally"
        (OUTPUT / "result.json").write_text(json.dumps({"platform": "Ubuntu X11 / Xfwm / software Mesa",
            "initial_geometry": initial, "checks": ["launch", "terminal fixture", "maximize", "restore", "caption double-click", "resize", "minimize", "close"]}, indent=2) + "\n")
        print("Linux desktop smoke passed: launch, maximize, restore, double-click, resize, minimize, close")
    finally:
        if app is not None and app.poll() is None:
            app.terminate()
            app.wait(timeout=15)
        if server_created:
            command("tmux", "-L", "desktop-smoke", "kill-server", check=False)
        wm.terminate()
        wm.wait(timeout=15)
        wm_log.close()
        app_log.close()
