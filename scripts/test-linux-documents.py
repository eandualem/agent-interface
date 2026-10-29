#!/usr/bin/env python3
"""Render a retained Document in a disposable Xvfb desktop and provider fixture.

Run under xvfb-run + dbus-run-session as a normal user. Never disables WebKit's
process sandbox. Uses no live provider, real home, credentials or tmux server.
"""
import csv
import hashlib
import http.server
import io
import json
import os
from pathlib import Path
import socketserver
import sqlite3
import subprocess
import tempfile
import threading
import time

ROOT = Path(__file__).resolve().parents[1]
SCALE = int(os.environ.get("DOCUMENT_TEST_SCALE", "100"))
assert SCALE in (100, 150)
OUTPUT = ROOT / "target/linux-documents-smoke" / str(SCALE)
OUTPUT.mkdir(parents=True, exist_ok=True)


def wait_for(check, label):
    deadline = time.monotonic() + 40
    while time.monotonic() < deadline:
        result = check()
        if result:
            return result
        time.sleep(0.3)
    raise AssertionError(f"Timed out: {label}")


with tempfile.TemporaryDirectory(prefix="document-smoke-") as directory:
    root = Path(directory)
    env = {k: v for k, v in os.environ.items()
           if k != "TMUX" and not k.startswith(("BACKBONE_", "AGENT_INTERFACE_", "TMUX_WORKSPACES_"))}
    env.pop("WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS", None)
    env.update(HOME=str(root), XDG_CONFIG_HOME=str(root / "config"),
               XDG_DATA_HOME=str(root / "data"), XDG_STATE_HOME=str(root / "state"),
               TMUX_TMPDIR=str(root / "tmux"), LIBGL_ALWAYS_SOFTWARE="1")
    (root / "tmux").mkdir(mode=0o700)
    settings = root / "config/agent-interface"
    settings.mkdir(parents=True)
    (settings / "settings.toml").write_text(f'interface_size = {SCALE}\n[setup]\nintro_seen = true\ndone = true\n')
    provider = root / "provider"
    provider.mkdir()
    (provider / "backbone.db").touch()
    env.update(BACKBONE_DATA_DIR=str(provider), BACKBONE_API_KEY="test-only",
               BACKBONE_SOCKET_PATH=str(root / "provider.sock"))
    leaks = []

    class Trap(http.server.BaseHTTPRequestHandler):
        def do_GET(self):
            leaks.append(self.path)
            self.send_response(200)
            self.end_headers()
        def log_message(self, *_):
            pass

    trap = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Trap)
    threading.Thread(target=trap.serve_forever, daemon=True).start()
    target = f'http://127.0.0.1:{trap.server_port}'
    html = '''<!doctype html><html><head><meta charset="utf-8"><style>
body { font: 24px sans-serif; padding: 24px; } button,input {font:inherit;margin:12px;}
</style></head><body><h1>DOCUMENTREADY</h1><p id="result">WAITING</p>
<button id="action" onclick="this.textContent='CLICKPASSED'">INTERACT</button>
<input aria-label="Input" placeholder="TYPEHERE"><input id="file" type="file" hidden>
<button onclick="document.getElementById('file').click();this.textContent='FILEBLOCKED'">FILETEST</button>
<p><a href="TARGET/navigation">NAVTEST</a> <button onclick="window.open('TARGET/popup');this.textContent='POPUPBLOCKED'">POPUPTEST</button></p>
<svg width="100" height="60"><rect width="100" height="60" fill="#3498db"/></svg>
<canvas id="chart" width="100" height="60"></canvas>
<script>
chart.getContext('2d').fillRect(0,0,80,50);
let rtc = false; try { new RTCPeerConnection(); rtc = true; } catch (_) {}
fetch('TARGET/fetch').then(()=>result.textContent='NETWORKFAILED',()=>result.textContent=rtc?'RTCFAILED':'ISOLATIONPASSED');
let image = new Image(); image.src='TARGET/image';
try { new WebSocket('TARGET/socket'.replace('http:', 'ws:')); } catch (_) {}
</script></body></html>'''.replace('TARGET', target)
    metadata = dict(id=1, document_id="d" * 32, author_id="a" * 32,
                    author_name="fixture", agent_name=None, title="Renderer fixture",
                    created_at="2026-01-01T00:00:00Z", content_bytes=len(html.encode()),
                    content_sha256=hashlib.sha256(html.encode()).hexdigest(),
                    supersedes_id=None, superseded_by=None)
    requests = []

    class Provider(http.server.BaseHTTPRequestHandler):
        def do_GET(self):
            requests.append(self.path)
            path = self.path.split('?', 1)[0]
            status = 200
            if path == '/api/documents/1':
                body = dict(metadata, html=html)
            elif path == '/api/documents':
                body = dict(items=[metadata], has_more=False, next_before_id=None)
            elif path.startswith('/api/conversations'):
                status, body = 404, {}
            else:
                body = dict(items=[], has_more=False, next_cursor=None)
            encoded = json.dumps(body).encode()
            self.send_response(status)
            self.send_header('Content-Type', 'application/json')
            self.send_header('Content-Length', str(len(encoded)))
            self.end_headers()
            self.wfile.write(encoded)
        def log_message(self, *_):
            pass

    class Server(socketserver.ThreadingUnixStreamServer):
        daemon_threads = True

    server = Server(str(root / 'provider.sock'), Provider)
    os.chmod(root / 'provider.sock', 0o600)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    library = root / 'library'
    library.mkdir()
    leaf = dict(id="000000000003", view="reports")
    model = dict(version=2, focus=False, selected="000000000001", workspaces=[
        dict(id="000000000001", name="Fixture", selected="000000000002", tabs=[
            dict(id="000000000002", name="Documents", focus=leaf['id'], tree=leaf)])])
    with sqlite3.connect(library / 'layouts.db') as db:
        db.execute('CREATE TABLE terminal_layout (id INTEGER PRIMARY KEY, value TEXT)')
        db.execute('INSERT INTO terminal_layout VALUES (1, ?)', (json.dumps(model),))

    def command(*args, check=True):
        return subprocess.run(args, env=env, text=True, capture_output=True,
                              timeout=15, check=check).stdout.strip()

    app = wm = None
    with (OUTPUT / 'app.log').open('w') as app_log, (OUTPUT / 'wm.log').open('w') as wm_log:
        try:
            wm = subprocess.Popen(['xfwm4', '--compositor=on', '--sm-client-disable'],
                                  env=env, stdout=wm_log, stderr=subprocess.STDOUT)
            wait_for(lambda: '_GTK_FRAME_EXTENTS' in command('xprop', '-root', '_NET_SUPPORTED'), 'window manager')
            app = subprocess.Popen([str(ROOT / 'target/debug/agent-interface'), '--data-dir', str(library)],
                                   env=env, stdout=app_log, stderr=subprocess.STDOUT)
            window = wait_for(lambda: command('xdotool', 'search', '--onlyvisible', '--pid', str(app.pid), check=False),
                              'app window').splitlines()[0]
            command('xdotool', 'windowactivate', '--sync', window)

            def words():
                assert app.poll() is None, 'application exited'
                command('import', '-window', window, str(OUTPUT / 'current.png'))
                tsv = command('tesseract', str(OUTPUT / 'current.png'), 'stdout', 'tsv')
                return list(csv.DictReader(io.StringIO(tsv), delimiter='\t'))

            def find(text, last=False):
                found = [word for word in words() if word.get('text', '').strip() == text]
                return (found[-1] if last else found[0]) if found else None

            def click(text, last=False):
                word = wait_for(lambda: find(text, last), text)
                x = int(word['left']) + int(word['width']) // 2
                y = int(word['top']) + int(word['height']) // 2
                command('xdotool', 'mousemove', '--window', window, str(x), str(y), 'click', '1')

            # The tab and the feed both say Documents; the feed is below the tab.
            click('Documents', last=True)
            wait_for(lambda: any(p.startswith('/api/documents?') for p in requests), 'document list request')
            click('Renderer')
            wait_for(lambda: find('DOCUMENTREADY'), 'rendered HTML')
            wait_for(lambda: find('ISOLATIONPASSED'), 'native WebRTC restriction and CSP')
            click('INTERACT')
            wait_for(lambda: find('CLICKPASSED'), 'JavaScript button')
            click('TYPEHERE')
            command('xdotool', 'type', '--clearmodifiers', 'INPUTPASSED')
            wait_for(lambda: find('INPUTPASSED'), 'HTML keyboard input')
            command('xdotool', 'key', '--clearmodifiers', 'ctrl+shift+k')
            wait_for(lambda: find('Settings'), 'app shortcut from HTML input')
            assert not find('DOCUMENTREADY'), 'webview must hide beneath native overlay'
            command('xdotool', 'key', 'Escape')
            wait_for(lambda: find('CLICKPASSED'), 'page restored after native overlay')
            native_windows = set(command('xdotool', 'search', '--onlyvisible', '--pid', str(app.pid)).splitlines())
            click('FILETEST')
            wait_for(lambda: find('FILEBLOCKED'), 'file chooser denial')
            assert set(command('xdotool', 'search', '--onlyvisible', '--pid', str(app.pid)).splitlines()) == native_windows, 'unexpected file dialog'
            click('NAVTEST')
            wait_for(lambda: find('DOCUMENTREADY'), 'external navigation denied')
            click('POPUPTEST')
            wait_for(lambda: find('POPUPBLOCKED'), 'new window denied')
            assert set(command('xdotool', 'search', '--onlyvisible', '--pid', str(app.pid)).splitlines()) == native_windows, 'unexpected popup'
            command('import', '-window', window, str(OUTPUT / 'document.png'))
            command('xdotool', 'windowsize', '--sync', window, '1100', '760')
            wait_for(lambda: find('CLICKPASSED'), 'state retained across resize')
            assert not leaks, f'document made network requests: {leaks}'
            command('import', '-window', window, str(OUTPUT / 'resized.png'))
            click('Documents', last=True)
            wait_for(lambda: find('Renderer'), 'return to document shelf')
            assert not find('DOCUMENTREADY'), 'closed document view still visible'
            (OUTPUT / 'result.json').write_text(json.dumps(dict(checks=[
                'Document API load', 'HTML render', 'inline script', 'HTML input', 'app shortcut from HTML',
                'overlay hides/restores view', 'return to shelf destroys view', 'file chooser denied',
                'WebRTC disabled', 'fetch/image/WebSocket blocked', 'external navigation/new windows denied', 'resize retains state'], requests=requests), indent=2))
            print('Linux Documents smoke passed')
        finally:
            if app and app.poll() is None:
                app.terminate()
                app.wait(timeout=15)
            if wm and wm.poll() is None:
                wm.terminate()
                wm.wait(timeout=15)
            server.shutdown()
            server.server_close()
            trap.shutdown()
            trap.server_close()
