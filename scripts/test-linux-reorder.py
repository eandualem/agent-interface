#!/usr/bin/env python3
"""Drag a tab and a workspace in the real Linux window and check the stored order.

Run with dbus-run-session and xvfb-run. The library, home and settings are
private to this test. Its panes are empty, so no shell or tmux server starts.
"""
import csv
import io
import json
import os
import re
from pathlib import Path
import sqlite3
import subprocess
import tempfile
import time
import tomllib

ROOT = Path(__file__).resolve().parents[1]
OUTPUT = ROOT / "target/linux-desktop-smoke/reorder"
OUTPUT.mkdir(parents=True, exist_ok=True)
TOKENS = tomllib.loads((ROOT / "crates/app/assets/design/tokens.toml").read_text())


def wait_for(check, description):
    deadline = time.monotonic() + 40
    while time.monotonic() < deadline:
        result = check()
        if result:
            return result
        time.sleep(0.3)
    raise AssertionError(f"Timed out: {description}")


def empty(id):
    return {"id": id, "agent": None, "empty": True}


def tab(id, name, leaf):
    return {"id": id, "name": name, "focus": leaf, "tree": empty(leaf)}


with tempfile.TemporaryDirectory(prefix="agent-interface-reorder-") as directory:
    root = Path(directory)
    env = {key: value for key, value in os.environ.items()
           if key != "TMUX" and not key.startswith(("BACKBONE_", "AGENT_INTERFACE_"))}
    env.update(HOME=str(root), XDG_CONFIG_HOME=str(root / "config"),
               XDG_DATA_HOME=str(root / "data"), XDG_STATE_HOME=str(root / "state"),
               TMUX_TMPDIR=str(root / "tmux"), LIBGL_ALWAYS_SOFTWARE="1")
    (root / "tmux").mkdir(mode=0o700)
    settings = root / "config/agent-interface"
    settings.mkdir(parents=True)
    (settings / "settings.toml").write_text('[setup]\nintro_seen = true\ndone = true\n')
    library = root / "library"
    library.mkdir()
    model = {"version": 2, "focus": False, "selected": "000000000001", "workspaces": [
        {"id": "000000000001", "name": "Alpha", "selected": "000000000002", "tabs": [
            tab("000000000002", "Apple", "000000000003"),
            tab("000000000004", "Banana", "000000000005"),
            tab("000000000006", "Cherry", "000000000007")]},
        {"id": "000000000008", "name": "Beta", "selected": "000000000009", "tabs": [
            tab("000000000009", "Damson", "000000000010")]}]}
    with sqlite3.connect(library / "layouts.db") as db:
        db.execute("CREATE TABLE terminal_layout (id INTEGER PRIMARY KEY, value TEXT)")
        db.execute("INSERT INTO terminal_layout VALUES (1, ?)", (json.dumps(model),))

    def stored():
        with sqlite3.connect(library / "layouts.db") as db:
            return json.loads(db.execute("SELECT value FROM terminal_layout WHERE id = 1").fetchone()[0])

    def command(*args, check=True):
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
            command("xdotool", "windowsize", "--sync", window, "1200", "820")

            def words():
                assert app.poll() is None, "application exited"
                command("import", "-window", window, str(OUTPUT / "current.png"))
                tsv = command("tesseract", str(OUTPUT / "current.png"), "stdout", "--psm", "11", "tsv")
                return list(csv.DictReader(io.StringIO(tsv), delimiter="\t"))

            def find(text):
                found = [word for word in words() if (word.get("text") or "").strip() == text]
                return found[0] if found else None

            def centre(word):
                return int(word["left"]) + int(word["width"]) // 2, int(word["top"]) + int(word["height"]) // 2

            def moves(start, end):
                (x0, y0), (x1, y1) = start, end
                steps = []
                for step in range(1, 11):
                    x = x0 + (x1 - x0) * step // 10
                    y = y0 + (y1 - y0) * step // 10
                    steps += ["mousemove", "--window", window, str(x), str(y), "sleep", "0.05"]
                return steps

            def drag(start, end, back=None, escape=False, capture=None):
                # Press, hold, then move in small steps the way a hand does, so
                # GPUI sees the drag begin past its threshold and follow the
                # pointer. Optionally press Escape halfway, come back, or keep
                # a capture of the held drag (the landing line and the lifted
                # copy) for the evidence folder.
                x0, y0 = start
                steps = ["mousemove", "--window", window, str(x0), str(y0), "mousedown", "1", "sleep", "0.3"]
                steps += moves(start, end)
                if escape:
                    steps += ["key", "Escape", "sleep", "0.2"]
                if back:
                    steps += moves(end, back)
                command("xdotool", *steps, "sleep", "0.3")
                if capture:
                    command("import", "-window", window, str(OUTPUT / capture))
                command("xdotool", "mouseup", "1")

            def tab_order():
                return [t["name"] for t in stored()["workspaces"][0]["tabs"]]

            row = TOKENS["size"]["row"]
            apple = wait_for(lambda: find("Apple"), "tab rows")
            banana = wait_for(lambda: find("Banana"), "second tab")
            cherry = wait_for(lambda: find("Cherry"), "third tab")
            x, y = centre(apple)
            above_apple = (x, y - round(row / 4))

            # Escape during a drag lets go: nothing moves.
            drag(centre(cherry), above_apple, escape=True)
            time.sleep(1)
            assert tab_order() == ["Apple", "Banana", "Cherry"], stored()
            # Banana dragged away and back onto its own name selects nothing;
            # the next save below stores the selection the window then has.
            drag(centre(banana), centre(cherry), back=centre(banana))
            time.sleep(1)
            assert tab_order() == ["Apple", "Banana", "Cherry"], stored()
            # Nor does one let go with Escape and released where it was pressed.
            drag(centre(banana), centre(cherry), escape=True, back=centre(banana))
            time.sleep(1)
            assert tab_order() == ["Apple", "Banana", "Cherry"], stored()

            # Cherry above Apple: release in the upper part of Apple's row.
            drag(centre(cherry), above_apple, capture="tab-drag-held.png")
            wait_for(lambda: tab_order() == ["Cherry", "Apple", "Banana"], "tab order saved")
            command("import", "-window", window, str(OUTPUT / "tab-moved.png"))

            # The second workspace slot before the first: release on the left
            # half of the first slot. Slots sit in the switcher row above the
            # status row, from the panel's left edge. With client-side
            # decorations the X window also holds the shadow GTK_FRAME_EXTENTS
            # names (left, right, top, bottom), so the content starts inside it.
            space, size = TOKENS["space"], TOKENS["size"]
            height = int(dict(line.split("=", 1) for line in
                              command("xdotool", "getwindowgeometry", "--shell", window).splitlines())["HEIGHT"])
            extents = re.findall(r"\d+", command("xprop", "-id", window, "_GTK_FRAME_EXTENTS", check=False).split("=")[-1])
            left, _, _, bottom = (int(value) for value in extents) if len(extents) == 4 else (0, 0, 0, 0)
            slot_y = height - bottom - round(size["status_row"] + size["slot_row"] / 2)
            first = left + round(space["s10"] + size["slot"] / 2)
            second = round(first + size["slot"] + space["s4"])
            drag((second, slot_y), (first - round(size["slot"] / 4), slot_y), capture="workspace-drag-held.png")
            wait_for(lambda: [w["name"] for w in stored()["workspaces"]] == ["Beta", "Alpha"], "workspace order saved")
            command("import", "-window", window, str(OUTPUT / "workspace-moved.png"))

            command("xdotool", "key", "--clearmodifiers", "ctrl+shift+q")
            assert app.wait(timeout=15) == 0, "application quit failed"
            saved = stored()
            assert [w["name"] for w in saved["workspaces"]] == ["Beta", "Alpha"], saved
            assert [t["name"] for t in saved["workspaces"][1]["tabs"]] == ["Cherry", "Apple", "Banana"], saved
            # No drag, cancelled drag or drag back changed the selection or
            # which pane of each tab has focus.
            assert saved["selected"] == "000000000001" and saved["workspaces"][1]["selected"] == "000000000002", saved
            def focus(layout):
                return {t["id"]: t["focus"] for w in layout["workspaces"] for t in w["tabs"]}
            assert focus(saved) == focus(model), saved
            (OUTPUT / "result.json").write_text(json.dumps({"checks": [
                "Escape cancels a drag", "drag back onto its own name selects nothing",
                "Escape and release at the press point selects nothing",
                "tab dragged above another", "workspace dragged before another",
                "order stored after quit", "selection unchanged", "pane focus unchanged"]}, indent=2) + "\n")
            print("Linux reorder smoke passed: tab and workspace drags stored in layouts.db")
        finally:
            if app and app.poll() is None and window:
                try:
                    command("import", "-window", window, str(OUTPUT / "failure.png"))
                except subprocess.SubprocessError:
                    pass
            if app and app.poll() is None:
                app.terminate()
                app.wait(timeout=15)
            if wm and wm.poll() is None:
                wm.terminate()
                wm.wait(timeout=15)
