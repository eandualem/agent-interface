#!/usr/bin/env python3
"""Click through first-agent selection and recovery in a private Linux desktop.

Uses the real native app and a deliberately small fake provider protocol. No
agent executable, model, real provider, user home or shared tmux server is used.
Run under xvfb-run + dbus-run-session, as a normal user.
"""
import csv
import http.server
import io
import json
import os
import re
from pathlib import Path
import shutil
import socketserver
import subprocess
import tempfile
import threading
import time
import tomllib

ROOT = Path(__file__).resolve().parents[1]
BINARY = Path(os.environ.get("ONBOARDING_TEST_BINARY", str(ROOT / "target/debug/agent-interface"))).resolve()
SCALE = int(os.environ.get("ONBOARDING_TEST_SCALE", "100"))
assert SCALE in (100, 150)
OUTPUT = ROOT / "target/linux-desktop-smoke/onboarding" / str(SCALE)
OUTPUT.mkdir(parents=True, exist_ok=True)
TOKENS = tomllib.loads((ROOT / "crates/app/assets/design/tokens.toml").read_text())
PALETTE = TOKENS["color"]["dark"]
accent = tuple(bytes.fromhex(PALETTE["accent"][1:]))
surface = tuple(bytes.fromhex(PALETTE["surface"][1:]))
DISABLED_ACCENT = "#" + "".join(f"{round((a + s) / 2):02x}" for a, s in zip(accent, surface))


def wait_for(check, description):
    deadline = time.monotonic() + 40
    while time.monotonic() < deadline:
        result = check()
        if result:
            return result
        time.sleep(0.3)
    raise AssertionError(f"Timed out: {description}")


with tempfile.TemporaryDirectory(prefix="onboarding-smoke-") as directory:
    root = Path(directory)
    env = {k: v for k, v in os.environ.items()
           if k != "TMUX" and not k.startswith(("BACKBONE_", "AGENT_INTERFACE_", "TMUX_WORKSPACES_"))}
    env.update(HOME=str(root), XDG_CONFIG_HOME=str(root / "config"),
               XDG_DATA_HOME=str(root / "data"), XDG_STATE_HOME=str(root / "state"),
               XDG_CACHE_HOME=str(root / "cache"), TMUX_TMPDIR=str(root / "tmux"),
               LIBGL_ALWAYS_SOFTWARE="1")
    (root / "tmux").mkdir(mode=0o700)
    settings = root / "config/agent-interface"
    settings.mkdir(parents=True)
    (settings / "settings.toml").write_text(f'theme = "dark"\ninterface_size = {SCALE}\n[setup]\nintro_seen = true\ndone = false\n')
    provider = root / "provider"
    provider.mkdir(mode=0o700)
    (provider / "backbone.db").touch()
    env.update(BACKBONE_DATA_DIR=str(provider), BACKBONE_API_KEY="fixture-only",
               BACKBONE_SOCKET_PATH=str(provider / "api.sock"))
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
    real_tmux = shutil.which("tmux")
    assert real_tmux
    executable("tmux", f'#!/bin/sh\nunset TMUX\nexec "{real_tmux}" -L onboarding-smoke "$@"\n')
    for name in ("codex", "claude", "opencode"):
        executable(name, "#!/bin/sh\nexit 99\n")  # No real agent can be launched.
    rows = []
    for name, label in (("codex", "Codex"), ("opencode", "OpenCode"),
                        ("claude", "Claude Code"), ("shell", "Plain shell")):
        row = dict(id=name, display_name=label, available=True, setup_supported=name != "shell",
                   models=[], authentication="unknown")
        if name in ("opencode", "shell"):
            row["setup_compatibility"] = dict(compatible=False,
                message="Old OpenCode fixture requires an update." if name == "opencode" else "First-agent setup is unavailable for this runtime.",
                path=str(bin_dir / name), version="1.19.5" if name == "opencode" else None)
        rows.append(row)
    (root / "catalog.json").write_text(json.dumps(dict(schema_version=1, items=rows)))
    executable("backbone", '''#!/usr/bin/python3
import json,os,sys
from pathlib import Path
root=Path(os.environ['HOME'])
if sys.argv[1:2]==['runtimes']:
    with (root/'discovery.jsonl').open('a') as f: f.write('{}\\n')
    print((root/'catalog.json').read_text());sys.exit(0)
if sys.argv[1:2]!=['setup']:
    print('agent-backbone 0.4.3');sys.exit(0)
r=json.loads(Path(sys.argv[sys.argv.index('--file')+1]).read_text())
with (root/'requests.jsonl').open('a') as f: f.write(json.dumps(r)+'\\n')
e=dict(schema_version=1,request_id=r['request_id'],seq=1,type='result',action=r['action'],completed_steps=[])
if r['action']=='cancel_first_agent':
    e.update(status='completed',code='request_cancelled',retry='new_request',
             data=dict(cancelled_request_id=r['request_id'],can_start_new_request=True));code=0
elif r['agent']['runtime']=='codex':
    e.update(status='action_required',code='runtime_permission',retry='same_request',
             message='Codex fixture needs sign-in.',data={});code=3
else:
    assert r['agent']['runtime']=='claude',r
    e.update(status='completed',code='agent_ready',retry='none',
             data=dict(agent_name=r['agent']['name'],ready='ready'));code=0
print(json.dumps(e));sys.exit(code)
''')

    class Provider(http.server.BaseHTTPRequestHandler):
        def do_GET(self):
            body = json.dumps(dict(items=[], has_more=False, next_cursor=None)).encode()
            self.send_response(200)
            self.send_header("Content-Length", str(len(body)))
            self.send_header("Content-Type", "application/json")
            self.end_headers()
            self.wfile.write(body)
        def log_message(self, *_):
            pass

    class Server(socketserver.ThreadingUnixStreamServer):
        daemon_threads = True

    server = Server(str(provider / "api.sock"), Provider)
    (provider / "api.sock").chmod(0o600)
    threading.Thread(target=server.serve_forever, daemon=True).start()

    def command(*args, check=True):
        return subprocess.run(args, env=env, text=True, capture_output=True,
                              timeout=15, check=check).stdout.strip()

    def requests():
        p = root / "requests.jsonl"
        return [json.loads(line) for line in p.read_text().splitlines()] if p.exists() else []

    app = wm = window = None
    with (OUTPUT / "app.log").open("w") as app_log, (OUTPUT / "wm.log").open("w") as wm_log:
        try:
            wm = subprocess.Popen(["xfwm4", "--compositor=on", "--sm-client-disable"],
                                  env=env, stdout=wm_log, stderr=subprocess.STDOUT)
            wait_for(lambda: "_GTK_FRAME_EXTENTS" in command("xprop", "-root", "_NET_SUPPORTED"), "window manager")

            def launch():
                global app, window
                app = subprocess.Popen([str(BINARY)],
                                       env=env, stdout=app_log, stderr=subprocess.STDOUT)
                window = wait_for(lambda: command("xdotool", "search", "--onlyvisible", "--pid", str(app.pid), check=False), "app window").splitlines()[0]
                command("xdotool", "windowactivate", "--sync", window)
                command("xdotool", "windowsize", "--sync", window,
                        str(round(1200 * SCALE / 100)), str(round(850 * SCALE / 100)))
                command("xdotool", "mousemove", "--window", window, "5", "5")
                time.sleep(0.2)

            def words():
                assert app.poll() is None, "application exited"
                command("import", "-window", window, str(OUTPUT / "current.png"))
                tsv = command("tesseract", str(OUTPUT / "current.png"), "stdout", "--psm", "11", "tsv")
                (OUTPUT / "current.tsv").write_text(tsv)
                found = [w for w in csv.DictReader(io.StringIO(tsv), delimiter="\t") if (w.get("text") or "").strip()]
                # Sparse-page OCR treats filled native buttons as illustrations.
                # Locate their actual painted fills, then read an inset text crop
                # as a single line. Coordinates still come from rendered pixels.
                components = command("convert", str(OUTPUT / "current.png"), "-fuzz", "3%",
                    "-fill", "white", "-opaque", PALETTE["accent"],
                    "-opaque", DISABLED_ACCENT, "-fuzz", "0%", "-opaque", PALETTE["field"],
                    "-fill", "black", "+opaque", "white",
                    "-define", "connected-components:verbose=true", "-connected-components", "8", "null:")
                for index, line in enumerate(components.splitlines()):
                    match = re.match(r"\s*\d+: (\d+)x(\d+)\+(\d+)\+(\d+) .*srgb\(255,255,255\)", line)
                    if not match:
                        continue
                    width, height, x, y = map(int, match.groups())
                    if width < 40 or height < 20:
                        continue
                    inset = round(4 * SCALE / 100)
                    crop = OUTPUT / f"button-{index}.png"
                    command("convert", str(OUTPUT / "current.png"), "-crop",
                        f"{width - 2 * inset}x{height - 2 * inset}+{x + inset}+{y + inset}",
                        "+repage", "-resize", "300%", str(crop))
                    button_tsv = command("tesseract", str(crop), "stdout", "--psm", "7", "tsv")
                    for word in csv.DictReader(io.StringIO(button_tsv), delimiter="\t"):
                        if not (word.get("text") or "").strip():
                            continue
                        for axis, origin in (("left", x + inset), ("top", y + inset)):
                            word[axis] = str(origin + int(word[axis]) // 3)
                        for dimension in ("width", "height"):
                            word[dimension] = str(max(1, int(word[dimension]) // 3))
                        found.append(word)
                return found

            def text():
                return " ".join(w["text"] for w in words())

            def click(label, last=False):
                def find():
                    found = [w for w in words() if w["text"].strip(".,:;‘’“”\"\'") == label]
                    found.sort(key=lambda w: (int(w["top"]), int(w["left"])))
                    return (found[-1] if last else found[0]) if found else None
                word = wait_for(find, label)
                command("xdotool", "mousemove", "--window", window,
                        str(int(word["left"]) + int(word["width"]) // 2),
                        str(int(word["top"]) + int(word["height"]) // 2))
                # Let the native window process the pointer enter/motion before
                # the press, including the first interaction after relaunch.
                time.sleep(0.2)
                command("xdotool", "click", "1")

            def capture(name):
                value = text()
                shutil.copyfile(OUTPUT / "current.png", OUTPUT / f"{name}.png")
                (OUTPUT / f"{name}.txt").write_text(value)
                return value

            def refresh_tools():
                # The refresh control is an icon at the right of the selected
                # tool's note. Anchor its row to rendered text and its right
                # inset to the same design tokens used by the native layout.
                note = wait_for(lambda: next((w for w in words() if w["text"] == "Uses"), None),
                                "selected tool note")
                geometry = dict(line.split("=", 1) for line in
                                command("xdotool", "getwindowgeometry", "--shell", window).splitlines())
                inset = (TOKENS["space"]["s24"] + TOKENS["size"]["icon_button"] / 2) * SCALE / 100
                command("xdotool", "mousemove", "--window", window,
                        str(round(int(geometry["WIDTH"]) - inset)),
                        str(int(note["top"]) + int(note["height"]) // 2))
                time.sleep(0.2)
                command("xdotool", "click", "1")

            def enter_first_agent():
                click("Continue")
                wait_for(lambda: "Copy" in text(), "Agent tools")
                click("Continue")

            launch()
            enter_first_agent()
            wait_for(lambda: "Choose an agent tool" in text(), "explicit selection")
            initial = capture("unselected")
            assert "Old OpenCode fixture" not in initial and "Plain shell" not in initial
            assert not requests()
            click("OpenCode")
            wait_for(lambda: "Old OpenCode fixture" in text(), "selected tool diagnosis")
            click("Choose", last=True)  # The incompatible choice must not submit anything.
            assert not requests()
            click("Codex")
            wait_for(lambda: "Start with Codex" in text(), "Codex choice")
            selected = capture("codex-selected")
            assert "Old OpenCode fixture" not in selected and "Plain shell" not in selected
            discoveries = len((root / "discovery.jsonl").read_text().splitlines())
            refresh_tools()
            wait_for(lambda: len((root / "discovery.jsonl").read_text().splitlines()) > discoveries, "discovery refresh")
            wait_for(lambda: "Start with Codex" in text(), "choice survives refresh")
            click("Start", last=True)
            wait_for(lambda: "Codex fixture needs sign-in" in text(), "Codex-only failure")
            first = requests()[0]
            assert first["agent"]["runtime"] == "codex" and first["hooks"] == ["codex"]
            capture("codex-failed")
            app.terminate()
            app.wait(timeout=15)
            launch()
            enter_first_agent()
            wait_for(lambda: "Change agent tool" in text(), "saved request recovery after relaunch")
            click("Change")
            wait_for(lambda: "Choose an agent tool" in text(), "cancelled request unlocks choice")
            cancelled = requests()[1]
            assert cancelled["action"] == "cancel_first_agent"
            assert cancelled["request_id"] == first["request_id"]
            assert cancelled["agent"] == first["agent"]
            click("Claude")
            wait_for(lambda: "Start with Claude Code" in text(), "replacement tool")
            discoveries = len((root / "discovery.jsonl").read_text().splitlines())
            refresh_tools()
            wait_for(lambda: len((root / "discovery.jsonl").read_text().splitlines()) > discoveries, "replacement discovery refresh")
            wait_for(lambda: "Start with Claude Code" in text(), "non-default choice survives refresh")
            click("Start", last=True)
            wait_for(lambda: "Your agent is ready" in text(), "replacement success")
            final = requests()[2]
            assert len(requests()) == 3
            assert final["request_id"] != first["request_id"]
            assert final["agent"]["runtime"] == "claude" and final["hooks"] == ["claude"]
            for field in ("name", "directory", "brief"):
                assert final["agent"][field] == first["agent"][field]
            capture("replacement-ready")
            click("Continue")
            wait_for(lambda: "Open Agent Interface" in text(), "Shortcuts after successful setup")
            capture("shortcuts")
            (OUTPUT / "requests.json").write_text(json.dumps(requests(), indent=2) + "\n")
            (OUTPUT / "result.json").write_text(json.dumps(dict(scale=SCALE, passed=True,
                checks=["explicit choice", "selected diagnosis only", "incompatible start blocked",
                        "refresh preserves choice", "Codex request excludes OpenCode",
                        "failure survives relaunch", "cancellation before replacement",
                        "new runtime and request ID", "choices preserved", "continue after success"]), indent=2) + "\n")
            print(f"Linux onboarding regression passed at {SCALE}%")
        finally:
            if app is not None and app.poll() is None:
                app.terminate()
                app.wait(timeout=15)
            if wm is not None:
                wm.terminate()
                wm.wait(timeout=15)
            command("tmux", "kill-server", check=False)  # Wrapper always targets our private server.
            server.shutdown()
            server.server_close()
