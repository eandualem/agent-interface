"""Exercise installer transactions with inert payloads in private homes."""

import hashlib
import http.server
import io
import json
import shlex
import shutil
import socket
import sys
import os
from pathlib import Path
import subprocess
import tarfile
import tempfile
import threading
import unittest

INSTALL = Path(__file__).resolve().parents[2] / "install.sh"
APP_ID = "io.github.eandualem.agent-interface"


class InstallTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.home = self.root / 'test home $user% "name"'
        self.home.mkdir()
        self.tools = self.root / "tools"
        self.tools.mkdir()
        self.command("uname", 'case "$1" in -s) echo Linux;; -m) echo x86_64;; esac')
        self.command("getconf", "echo glibc 2.39")
        self.env = dict(os.environ, HOME=str(self.home), PATH=str(self.tools) + os.pathsep + os.environ["PATH"])
        self.env.pop("XDG_DATA_HOME", None)
        self.env.pop("WSL_DISTRO_NAME", None)
        self.installer = INSTALL
        self.destination = self.home / ".local/lib/agent-interface"
        self.launcher = self.home / f".local/share/applications/{APP_ID}.desktop"
        self.link = self.home / ".local/bin/agent-interface"

    def command(self, name, body):
        path = self.tools / name
        path.write_text("#!/bin/sh\n" + body + "\n")
        path.chmod(0o755)

    def package(self, version="0.0.0", target="x86_64-unknown-linux-gnu", extra=None, legacy_mac=False, legacy=False):
        archive = self.root / "app.tar.gz"
        mac = target.endswith("apple-darwin")
        name = "tmux-workspaces" if legacy else "agent-interface"
        bundle = "tmux-workspaces.app" if legacy_mac else "Agent Interface.app"
        root = bundle + "/Contents" if mac else name
        resources = root + "/Resources" if mac else root
        executable = root + ("/MacOS" if mac else "/bin") + "/" + name
        files = {resources + "/INSTALLER": "io.github.eandualem." + name + "\n",
                 resources + "/TARGET": target + "\n",
                 executable: f"#!/bin/sh\necho '{name} {version}'\n"}
        if not mac:
            files[root + "/agent-interface.png"] = "fixture icon"
        if extra:
            files.update(extra)
        with tarfile.open(archive, "w:gz") as tar:
            for name, text in files.items():
                info = tarfile.TarInfo(name)
                data = text.encode()
                info.size = len(data)
                info.mode = 0o755 if name == executable else 0o644
                tar.addfile(info, io.BytesIO(data))
        archive.with_suffix(".gz.sha256").write_text(hashlib.sha256(archive.read_bytes()).hexdigest() + "  app.tar.gz\n")
        return archive

    def install(self, archive, success=True):
        args = ["--archive", str(archive)] if archive else []
        result = subprocess.run(["sh", str(self.installer), *args], env=self.env, capture_output=True, text=True, timeout=60)
        self.assertEqual(result.returncode == 0, success, result.stdout + result.stderr)
        return result

    def wsl(self):
        # Redirect the one system path into the fixture. sudo is always inert;
        # these tests never write system launchers or install real packages.
        self.system_apps = self.root / "system-applications"
        self.system_apps.mkdir()
        self.installer = self.root / "install.sh"
        self.installer.write_text(INSTALL.read_text().replace(
            "/usr/share/applications", str(self.system_apps)))
        self.env.update(WSL_DISTRO_NAME="Ubuntu", DISPLAY=":0",
                        TEST_SYSTEM_ACTIONS=str(self.root / "system-actions"))
        self.command("dpkg-query", "exit 1")
        self.command("apt-get", "exit 99")
        self.command("sudo", '''
printf '%s\\n' "$*" >> "$TEST_SYSTEM_ACTIONS"
case "$1" in
    -v) exit 0;;
    apt-get) exit 0;;
    install)
        if [ "${TEST_FAIL_SHORTCUT:-}" = yes ]; then exit 1; fi
        shift; exec /usr/bin/install "$@";;
    rm) shift; exec /bin/rm "$@";;
    *) exit 99;;
esac
''')
        return self.system_apps / f"{APP_ID}.desktop"

    def sandbox_fixture(self):
        policy = self.root / "apparmor"
        policy.mkdir()
        restriction = self.root / "restrict-userns"
        restriction.write_text("1\n")
        parser = self.tools / "apparmor_parser"
        self.command("apparmor_parser", 'exit "${TEST_PARSER_STATUS:-0}"')
        self.sandbox_profile = policy / "agent-interface-userns"
        self.installer = self.root / "install-sandbox.sh"
        self.installer.write_text(INSTALL.read_text()
            .replace("/proc/sys/kernel/apparmor_restrict_unprivileged_userns", str(restriction))
            .replace("/sys/kernel/security/apparmor", str(policy))
            .replace("/etc/apparmor.d", str(policy))
            .replace("/usr/sbin/apparmor_parser", str(parser)))
        self.env["TEST_SYSTEM_ACTIONS"] = str(self.root / "sandbox-actions")
        self.command("sudo", """
printf '%s\\n' "$*" >> "$TEST_SYSTEM_ACTIONS"
[ "$1" != -v ] || exit 0
exec "$@"
""")
        profile = (INSTALL.parent / "packaging/agent-interface.apparmor").read_text()
        return self.package(extra={"agent-interface/agent-interface.apparmor": profile})

    def test_sandbox_profile_is_scoped_and_loaded(self):
        archive = self.sandbox_fixture()
        self.install(archive)
        profile = self.sandbox_profile.read_text()
        self.assertIn("@{HOME}/.local/lib/agent-interface/bin/agent-interface", profile)
        self.assertIn("userns,", profile)
        actions = Path(self.env["TEST_SYSTEM_ACTIONS"]).read_text()
        self.assertIn("apparmor_parser -r", actions)
        self.assertNotIn("sysctl", actions)

    def test_sandbox_custom_profile_is_preserved(self):
        archive = self.sandbox_fixture()
        self.sandbox_profile.write_text("custom policy\n")
        result = self.install(archive, success=False)
        self.assertEqual(self.sandbox_profile.read_text(), "custom policy\n")
        self.assertIn("custom Agent Interface AppArmor profile was preserved", result.stderr)
        self.assertFalse(self.destination.exists())

    def test_sandbox_parser_failure_keeps_previous_app(self):
        self.install(self.package(version="old"))
        archive = self.sandbox_fixture()
        self.env["TEST_PARSER_STATUS"] = "1"
        result = self.install(archive, success=False)
        self.assertIn("Could not load", result.stderr)
        self.assertFalse(self.sandbox_profile.exists())
        self.assertIn("old", (self.destination / "bin/agent-interface").read_text())

    def test_wsl_installs_runtime_and_registers_start_menu_entry(self):
        shortcut = self.wsl()
        result = self.install(self.package())
        self.assertEqual(shortcut.read_bytes(), self.launcher.read_bytes())
        self.assertEqual(shortcut.stat().st_mode & 0o777, 0o644)
        self.assertIn("Windows Start", result.stdout)
        actions = (self.root / "system-actions").read_text()
        self.assertIn("apt-get update", actions)
        self.assertIn("apt-get install -y tmux git", actions)
        self.command("dpkg-query", "printf 'install ok installed'")
        (self.root / "system-actions").write_text("")
        self.install(self.package("0.0.1"))
        self.assertNotIn("apt-get", (self.root / "system-actions").read_text())

    def test_wsl_shortcut_failure_rolls_back_app(self):
        self.install(self.package("0.0.0"))
        before = self.launcher.read_bytes()
        shortcut = self.wsl()
        self.env["TEST_FAIL_SHORTCUT"] = "yes"
        self.install(self.package("0.0.1"), success=False)
        self.assertFalse(shortcut.exists())
        self.assertEqual(self.launcher.read_bytes(), before)
        self.assertEqual(subprocess.check_output([self.link], text=True).strip(),
                         "agent-interface 0.0.0")

    def test_wsl_preserves_unowned_system_launcher(self):
        shortcut = self.wsl()
        shortcut.write_text("[Desktop Entry]\nName=Another installation\n")
        self.install(self.package(), success=False)
        self.assertEqual(shortcut.read_text(), "[Desktop Entry]\nName=Another installation\n")
        self.assertFalse((self.root / "system-actions").exists())

    def mock_download(self, archive, prefix="tmux-workspaces", missing_status="404"):
        self.env.update(TEST_ARCHIVE=str(archive), TEST_PREFIX=prefix,
                        TEST_DOWNLOADS=str(self.root / "downloads"), TEST_MISSING_STATUS=missing_status)
        self.command("curl", '''
printf '%s\\n' "$*" >> "$TEST_DOWNLOADS.args"
url= output= write_status=false
while [ "$#" -gt 0 ]; do
    case "$1" in
        -o) output=$2; shift 2;;
        --write-out) write_status=true; shift 2;;
        https://*) url=$1; shift;;
        *) shift;;
    esac
done
case "$url" in */releases/latest) printf 'https://github.com/eandualem/agent-interface/releases/tag/vfixture'; exit 0;; esac
name=${url##*/}
printf '%s\\n' "$name" >> "$TEST_DOWNLOADS"
case "$name" in
    "$TEST_PREFIX"-*.tar.gz) cp "$TEST_ARCHIVE" "$output";;
    "$TEST_PREFIX"-*.tar.gz.sha256) cp "$TEST_ARCHIVE.sha256" "$output";;
    *) if [ "$write_status" = true ]; then printf '%s' "$TEST_MISSING_STATUS"; fi; exit 22;;
esac
if [ "$write_status" = true ]; then printf 200; fi
''')

    def test_download_legacy_linux_then_update_to_renamed_release(self):
        archive = self.package("0.1.0", legacy=True)
        self.mock_download(archive)
        self.install(None)
        self.assertEqual(subprocess.check_output([self.link, "--version"], text=True).strip(), "tmux-workspaces 0.1.0")
        self.assertIn(f"Icon={self.destination}/agent-interface.png\n", self.launcher.read_text())
        self.assertTrue((self.destination / "agent-interface.png").is_file())
        self.assertIn("StartupWMClass=io.github.eandualem.tmux-workspaces\n", self.launcher.read_text())
        self.assertEqual((self.root / "downloads").read_text().splitlines(), [
            "agent-interface-x86_64-unknown-linux-gnu.tar.gz.sha256",
            "tmux-workspaces-x86_64-unknown-linux-gnu.tar.gz.sha256",
            "tmux-workspaces-x86_64-unknown-linux-gnu.tar.gz"])
        requests = (self.root / "downloads.args").read_text().splitlines()
        self.assertIn("--progress-bar", requests[3])
        archive = self.package("0.1.1")
        self.mock_download(archive, prefix="agent-interface")
        self.install(None)
        self.assertEqual(subprocess.check_output([self.link, "--version"], text=True).strip(), "agent-interface 0.1.1")
        self.assertIn("StartupWMClass=io.github.eandualem.agent-interface\n", self.launcher.read_text())

    def test_download_legacy_mac_then_update_to_renamed_release(self):
        self.command("uname", 'case "$1" in -s) echo Darwin;; -m) echo arm64;; esac')
        self.command("codesign", "exit 0")
        archive = self.package("0.1.0", target="aarch64-apple-darwin", legacy=True)
        self.mock_download(archive)
        self.install(None)
        destination = self.home / "Applications/Agent Interface.app/Contents/MacOS"
        self.assertEqual(subprocess.check_output([destination / "tmux-workspaces", "--version"], text=True).strip(), "tmux-workspaces 0.1.0")
        self.install(self.package("0.1.1", target="aarch64-apple-darwin"))
        self.assertEqual(subprocess.check_output([destination / "agent-interface", "--version"], text=True).strip(), "agent-interface 0.1.1")

    def test_download_failure_does_not_fall_back_or_change_installation(self):
        self.install(self.package("0.1.1"))
        self.mock_download(self.package("0.1.0", legacy=True), missing_status="500")
        self.install(None, success=False)
        self.assertEqual(subprocess.check_output([self.link, "--version"], text=True).strip(), "agent-interface 0.1.1")
        self.assertEqual((self.root / "downloads").read_text().splitlines(), ["agent-interface-x86_64-unknown-linux-gnu.tar.gz.sha256"])

    def test_legacy_download_checksum_failure_preserves_installation(self):
        self.install(self.package("0.1.1"))
        archive = self.package("0.1.0", legacy=True)
        with archive.open("ab") as stream:
            stream.write(b"corrupt")
        self.mock_download(archive)
        self.command("sleep", 'case "$1" in 2|4|8|16) exit 0;; *) exec /bin/sleep "$@";; esac')
        self.install(None, success=False)
        self.assertEqual(subprocess.check_output([self.link, "--version"], text=True).strip(), "agent-interface 0.1.1")

    def unreliable_download(self, archive, outcomes):
        self.env.update(TEST_ARCHIVE=str(archive), TEST_OUTCOMES=','.join(outcomes),
                        TEST_TRANSFERS=str(self.root / "transfers"))
        self.command("sleep", 'case "$1" in 2|4|8|16) exit 0;; *) exec /bin/sleep "$@";; esac')
        stub = self.root / "download.py"
        stub.write_text('''import json, os, sys
from pathlib import Path
args = sys.argv[1:]
output = Path(args[args.index('-o') + 1])
url = next(arg for arg in args if arg.startswith('https://'))
archive = Path(os.environ['TEST_ARCHIVE'])
if url.endswith('/releases/latest'):
    print('https://github.com/eandualem/agent-interface/releases/tag/vfixture', end='')
    sys.exit(0)
assert '/releases/download/vfixture/' in url
if url.endswith('.sha256'):
    output.write_bytes(Path(str(archive) + '.sha256').read_bytes())
    print('200', end='')
    sys.exit(0)
assert '--continue-at' in args and args[args.index('--continue-at') + 1] == '-'
assert '--max-time' not in args
assert args[args.index('--speed-limit') + 1] == '1'
assert args[args.index('--speed-time') + 1] == '60'
log = Path(os.environ['TEST_TRANSFERS'])
records = json.loads(log.read_text()) if log.exists() else []
offset = output.stat().st_size if output.exists() else 0
records.append(offset)
log.write_text(json.dumps(records))
plan = os.environ['TEST_OUTCOMES'].split(',')
outcome = plan[min(len(records) - 1, len(plan) - 1)]
if outcome in ('33', '416', '503', '404'):
    print(outcome if outcome != '33' else '200', end='')
    sys.exit(33 if outcome == '33' else 0 if outcome == '416' else 22)
data = archive.read_bytes()
if outcome == 'corrupt':
    output.write_bytes(b'bad archive')
else:
    end = len(data) if outcome == 'ok' else min(len(data) - 1, offset + 17)
    with output.open('ab') as stream:
        stream.write(data[offset:end])
print('206' if offset else '200', end='')
sys.exit(0 if outcome in ('ok', 'corrupt') else int(outcome))
''')
        self.command("curl", f"exec {shlex.quote(sys.executable)} {shlex.quote(str(stub))} \"$@\"")

    def real_curl_resume(self, reject_range=False):
        archive = self.package()
        body = archive.read_bytes()
        checksum = Path(str(archive) + '.sha256').read_bytes()
        offsets = []
        heads = []

        class Handler(http.server.BaseHTTPRequestHandler):
            def log_message(self, *args):
                pass

            def do_HEAD(self):
                heads.append(self.path)
                if self.path == '/releases/latest':
                    self.send_response(302)
                    self.send_header('Location', '/releases/tag/vfixture')
                elif self.path == '/releases/tag/vfixture':
                    self.send_response(200)
                else:
                    self.send_response(404)
                self.send_header('Content-Length', '0')
                self.end_headers()

            def do_GET(self):
                if not self.path.startswith('/releases/download/vfixture/'):
                    self.send_error(404)
                    return
                if self.path.endswith('.sha256'):
                    self.send_response(200)
                    self.send_header('Content-Length', str(len(checksum)))
                    self.end_headers()
                    self.wfile.write(checksum)
                    return
                offset = int(self.headers.get('Range', 'bytes=0-')[6:-1])
                offsets.append(offset)
                if reject_range and len(offsets) == 2:
                    self.send_response(416)
                    self.send_header('Content-Range', f'bytes */{len(body)}')
                    self.send_header('Content-Length', '0')
                    self.end_headers()
                    return
                self.send_response(206 if offset else 200)
                self.send_header('Content-Length', str(len(body) - offset))
                if offset:
                    self.send_header('Content-Range', f'bytes {offset}-{len(body) - 1}/{len(body)}')
                self.end_headers()
                if len(offsets) == 1:
                    self.wfile.write(body[:37])
                    self.wfile.flush()
                    self.connection.shutdown(socket.SHUT_RDWR)
                    self.connection.close()
                else:
                    self.wfile.write(body[offset:])

        server = http.server.HTTPServer(('127.0.0.1', 0), Handler)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        real_curl = shutil.which('curl')
        self.assertIsNotNone(real_curl)
        proxy = self.root / 'curl-proxy.py'
        # Only this fixture routes GitHub requests through its local HTTP
        # server. Real curl resolves HEAD redirects and transfers the archive;
        # translate its effective local URL back for production's origin check.
        local = f'http://127.0.0.1:{server.server_port}'
        repository = 'https://github.com/eandualem/agent-interface'
        proxy.write_text('import subprocess, sys\n' +
                         f'local, repository = {local!r}, {repository!r}\n' +
                         'args = [arg.replace("=https", "=http").replace(repository, local) for arg in sys.argv[1:]]\n' +
                         f'result = subprocess.run([{real_curl!r}, "--noproxy", "*", *args], stdout=subprocess.PIPE)\n' +
                         'sys.stdout.buffer.write(result.stdout.replace(local.encode(), repository.encode()))\n' +
                         'sys.exit(result.returncode)\n')
        self.command('curl', f'exec {shlex.quote(sys.executable)} {shlex.quote(str(proxy))} "$@"')
        try:
            result = self.install(None)
            if reject_range:
                self.assertIn('Server could not resume', result.stderr)
            self.assertEqual(heads, ['/releases/latest', '/releases/tag/vfixture'])
            self.assertEqual(offsets, [0, 37, 0] if reject_range else [0, 37])
            self.assertEqual((self.destination.parent / '.agent-interface-download/archive.part').read_bytes(), body)
        finally:
            server.shutdown()
            server.server_close()
            thread.join(timeout=5)

    def test_real_curl_resumes_a_truncated_http_response(self):
        self.real_curl_resume()

    def test_real_curl_range_rejection_starts_fresh(self):
        self.real_curl_resume(reject_range=True)

    def test_download_resumes_after_reset_and_stall(self):
        self.unreliable_download(self.package(), ['56', '28', 'ok'])
        result = self.install(None)
        self.assertEqual(json.loads((self.root / 'transfers').read_text()), [0, 17, 34])
        self.assertIn('attempt 3/5; 34 bytes saved', result.stderr)
        self.assertTrue(self.link.exists())

    def test_download_exhaustion_retains_bytes_for_next_installer_run(self):
        self.wsl()
        self.unreliable_download(self.package(), ['56'] * 5 + ['ok'])
        result = self.install(None, success=False)
        self.assertIn('rerun the installer to resume', result.stderr)
        self.assertFalse((self.root / 'system-actions').exists())
        self.assertFalse(self.destination.exists())
        self.install(None)
        self.assertEqual(json.loads((self.root / 'transfers').read_text()), [0, 17, 34, 51, 68, 85])
        self.assertIn('-v', (self.root / 'system-actions').read_text())
        # A verified archive is also reused if a later installation step failed.
        self.install(None)
        self.assertEqual(len(json.loads((self.root / 'transfers').read_text())), 6)

    def test_changed_release_does_not_resume_old_bytes(self):
        self.unreliable_download(self.package('0.0.0'), ['56'] * 5 + ['ok'])
        self.install(None, success=False)
        self.package('0.0.1')
        self.install(None)
        self.assertEqual(json.loads((self.root / 'transfers').read_text())[-1], 0)
        self.assertEqual(subprocess.check_output([self.link], text=True).strip(), 'agent-interface 0.0.1')

    def test_download_recovers_when_server_rejects_range(self):
        for rejection in ('33', '416'):
            with self.subTest(rejection=rejection):
                archive = self.package('0.0.' + rejection)
                (self.root / 'transfers').unlink(missing_ok=True)
                self.unreliable_download(archive, ['56', rejection, 'ok'])
                self.install(None)
                self.assertEqual(json.loads((self.root / 'transfers').read_text()), [0, 17, 0])

    def test_download_retries_server_error_and_corrupt_success(self):
        self.unreliable_download(self.package(), ['503', 'corrupt', 'ok'])
        self.install(None)
        self.assertEqual(json.loads((self.root / 'transfers').read_text()), [0, 0, 0])

    def test_download_missing_asset_does_not_loop_or_claim_network_failure(self):
        self.unreliable_download(self.package(), ['404'])
        result = self.install(None, success=False)
        self.assertIn('HTTP 404', result.stderr)
        self.assertEqual(json.loads((self.root / 'transfers').read_text()), [0])
        self.assertFalse(self.destination.exists())

    def test_release_resolution_requires_a_tag_in_the_same_repository(self):
        self.command('curl', "printf 'https://example.invalid/releases/tag/vfixture'")
        result = self.install(None, success=False)
        self.assertIn('did not resolve to a release tag', result.stderr)
        self.assertFalse((self.destination.parent / '.agent-interface-download').exists())

    def test_download_cache_rejects_symlink(self):
        archive = self.package()
        self.unreliable_download(archive, ['ok'])
        parent = self.destination.parent
        parent.mkdir(parents=True)
        cache = parent / '.agent-interface-download'
        cache.mkdir()
        outside = self.root / 'unrelated'
        outside.write_text('preserve')
        (cache / 'archive.part').symlink_to(outside)
        self.install(None, success=False)
        self.assertEqual(outside.read_text(), 'preserve')
        self.assertFalse((self.root / 'transfers').exists())

    def test_transitional_link_is_restored_after_update_failure(self):
        self.install(self.package("0.1.0", legacy=True))
        before = self.launcher.read_bytes()
        self.command("mv", 'for last do :; done\ncase "$last" in *.desktop)\n'
                     'case "$*" in *launcher.*) echo partial > "$last"; exit 1;; esac;; esac\n'
                     'exec /bin/mv "$@"')
        self.install(self.package("0.1.1"), success=False)
        self.assertEqual(subprocess.check_output([self.link, "--version"], text=True).strip(), "tmux-workspaces 0.1.0")
        self.assertEqual(self.launcher.read_bytes(), before)

    def test_install_update_and_preserve_data(self):
        library = self.home / ".local/share/agent-interface/layouts.db"
        library.parent.mkdir(parents=True)
        library.write_bytes(b"preserve")
        self.install(self.package())
        self.assertEqual(subprocess.check_output([self.link, "--version"], text=True).strip(), "agent-interface 0.0.0")
        self.assertIn(f"Icon={self.destination}/agent-interface.png\n", self.launcher.read_text())
        self.install(self.package("0.0.1"))
        self.assertEqual(subprocess.check_output([self.link, "--version"], text=True).strip(), "agent-interface 0.0.1")
        self.assertEqual(library.read_bytes(), b"preserve")
        # Exec quoting must survive desktop string unescaping before argument parsing.
        entry = next(line[5:] for line in self.launcher.read_text().splitlines() if line.startswith("Exec="))
        entry = entry.replace("\\\\", "\\")
        import shlex
        path = shlex.split(entry)[0].replace("\\$", "$").replace("%%", "%")
        self.assertEqual(Path(path).resolve(), self.link.resolve())

    def legacy_linux_install(self):
        old = self.home / ".local/lib/tmux-workspaces"
        binary = old / "bin/tmux-workspaces"
        binary.parent.mkdir(parents=True)
        binary.write_text("#!/bin/sh\necho 'tmux-workspaces 0.1.0'\n")
        binary.chmod(0o755)
        (old / "INSTALLER").write_text("io.github.eandualem.tmux-workspaces\n")
        link = self.home / ".local/bin/tmux-workspaces"
        link.parent.mkdir(parents=True)
        link.symlink_to(binary)
        launcher = self.launcher.with_name("io.github.eandualem.tmux-workspaces.desktop")
        launcher.parent.mkdir(parents=True)
        launcher.write_text("[Desktop Entry]\nX-TmuxWorkspaces-Installer=1\n")
        library = self.home / ".local/share/tmux-workspaces/layouts.db"
        library.parent.mkdir(parents=True)
        library.write_bytes(b"existing workspace")
        return old, link, launcher, library

    def test_linux_rename_retires_owned_launchers_and_preserves_library(self):
        old, link, launcher, library = self.legacy_linux_install()
        self.install(self.package())
        for retired in (old, link, launcher):
            self.assertFalse(retired.exists())
            self.assertFalse(retired.is_symlink())
        self.assertEqual(subprocess.check_output([self.link, "--version"], text=True).strip(), "agent-interface 0.0.0")
        self.assertEqual(library.read_bytes(), b"existing workspace")

    def test_linux_rename_failure_restores_old_install_and_launchers(self):
        old, link, launcher, library = self.legacy_linux_install()
        self.command("rm", 'for last do :; done\ncase "$last" in *tmux-workspaces.desktop) exit 1;; esac\nexec /bin/rm "$@"')
        self.install(self.package(), success=False)
        self.assertTrue(old.exists())
        self.assertTrue(launcher.exists())
        self.assertEqual(subprocess.check_output([link, "--version"], text=True).strip(), "tmux-workspaces 0.1.0")
        self.assertEqual(library.read_bytes(), b"existing workspace")
        self.assertFalse(self.destination.exists())
        self.assertFalse(self.link.is_symlink())
        self.assertFalse(self.launcher.exists())

    def test_reject_corrupt_wrong_target_and_unsafe_archive(self):
        archive = self.package()
        with archive.open("ab") as stream:
            stream.write(b"corrupt")
        self.install(archive, success=False)
        self.assertFalse(self.destination.exists())
        self.install(self.package(target="aarch64-unknown-linux-gnu"), success=False)
        self.assertFalse(self.destination.exists())
        self.install(self.package(extra={"agent-interface/../../escape": "no"}), success=False)
        self.assertFalse((self.root / "escape").exists())
        self.assertFalse(self.destination.exists())

    def test_refuse_unmanaged_installation(self):
        self.destination.mkdir(parents=True)
        (self.destination / "mine").write_text("preserve")
        self.install(self.package(), success=False)
        self.assertEqual((self.destination / "mine").read_text(), "preserve")

    def test_mac_rename_update_and_rollback(self):
        self.command("uname", 'case "$1" in -s) echo Darwin;; -m) echo arm64;; esac')
        self.command("codesign", "exit 0")
        destination = self.home / "Applications/Agent Interface.app"
        legacy = self.home / "Applications/tmux-workspaces.app"
        self.install(self.package(target="aarch64-apple-darwin", legacy_mac=True))
        destination.rename(legacy)
        # Failed replacement restores the old bundle under its original name.
        self.command("mv", 'case "$1" in */"Agent Interface.app") exit 1;; esac\nexec /bin/mv "$@"')
        self.install(self.package("0.0.1", target="aarch64-apple-darwin"), success=False)
        self.assertTrue(legacy.exists())
        self.assertFalse(destination.exists())
        (self.tools / "mv").unlink()
        self.install(self.package("0.0.1", target="aarch64-apple-darwin"))
        self.assertFalse(legacy.exists())
        binary = destination / "Contents/MacOS/agent-interface"
        self.assertEqual(subprocess.check_output([binary, "--version"], text=True).strip(), "agent-interface 0.0.1")
        self.install(self.package("0.0.2", target="aarch64-apple-darwin"))
        self.assertEqual(subprocess.check_output([binary, "--version"], text=True).strip(), "agent-interface 0.0.2")

    def test_mac_preserves_unmanaged_legacy_app(self):
        self.command("uname", 'case "$1" in -s) echo Darwin;; -m) echo arm64;; esac')
        legacy = self.home / "Applications/tmux-workspaces.app"
        legacy.mkdir(parents=True)
        (legacy / "mine").write_text("preserve")
        self.install(self.package(target="aarch64-apple-darwin"), success=False)
        self.assertEqual((legacy / "mine").read_text(), "preserve")
        self.assertFalse((self.home / "Applications/Agent Interface.app").exists())

    def test_rollback_after_launcher_failure(self):
        self.install(self.package())
        before = self.launcher.read_bytes()
        # Model a cross-filesystem move that damages the destination before failing.
        self.command("mv", 'for last do :; done\ncase "$last" in *.desktop)\n'
                     'case "$*" in *launcher.*) echo partial > "$last"; exit 1;; esac;; esac\n'
                     'exec /bin/mv "$@"')
        self.install(self.package("0.0.1"), success=False)
        self.assertEqual(subprocess.check_output([self.link, "--version"], text=True).strip(), "agent-interface 0.0.0")
        self.assertEqual(self.launcher.read_bytes(), before)

    def test_preserve_launcher_backup_when_restore_fails(self):
        self.install(self.package())
        before = self.launcher.read_bytes()
        self.command("mv", 'for last do :; done\ncase "$last" in *.desktop) '
                     'echo partial > "$last"; exit 1;; esac\nexec /bin/mv "$@"')
        self.install(self.package("0.0.1"), success=False)
        self.assertEqual(subprocess.check_output([self.link, "--version"], text=True).strip(), "agent-interface 0.0.0")
        backups = list(self.launcher.parent.glob(".agent-interface-previous.*"))
        self.assertEqual(len(backups), 1)
        self.assertEqual(backups[0].read_bytes(), before)


    def backbone_tools(self):
        self.env.update(FIXTURE_TOOLS=str(self.root / "uv tools"),
                        FIXTURE_BIN=str(self.home / ".local/bin"),
                        FIXTURE_LOG=str(self.root / "provider.log"),
                        FIXTURE_PYTHON=str(self.tools / "provider-python"))
        self.command("provider-python", '''
printf '%s\\n' "$*" >> "$FIXTURE_LOG"
case "$*" in *--resume-pending*) exit "${FIXTURE_RESUME:-0}";; esac
case ":$PATH:" in *":$FIXTURE_BIN:"*) :;; *) exit 89;; esac
if [ "${FIXTURE_SETUP_DELAY:-0}" != 0 ]; then sleep "$FIXTURE_SETUP_DELAY"; fi
exit "${FIXTURE_SETUP_FAIL:-0}"
''')
        self.command("uv", '''
printf '%s\\n' "$*" >> "$FIXTURE_LOG"
case "$1 $2" in
    "tool dir")
        if [ "${3:-}" = --bin ]; then printf '%s\\n' "$FIXTURE_BIN";
        else printf '%s\\n' "$FIXTURE_TOOLS"; fi;;
    "tool install")
        [ "${FIXTURE_INSTALL_FAIL:-0}" = 0 ] || exit 1
        mkdir -p "$FIXTURE_TOOLS/agent-backbone/bin"
        cp "$FIXTURE_PYTHON" "$FIXTURE_TOOLS/agent-backbone/bin/python"
        chmod 755 "$FIXTURE_TOOLS/agent-backbone/bin/python";;
    "tool update-shell")
        if [ "${FIXTURE_SHELL_FAIL:-0}" != 0 ]; then
            echo 'error: PATH is stale but the Bash configuration files are already up-to-date' >&2
            exit 1
        fi
        echo 'Restart your shell to apply changes' >&2;;
    *) exit 1;;
esac
''')

    def test_latest_provider_install_and_pending_resume_order(self):
        self.backbone_tools()
        archive = self.package("0.2.0", extra={"agent-interface/setup_backbone.py": "fixture helper"})
        first = self.install(archive)
        self.assertIn('Backbone command: "' + self.env["FIXTURE_BIN"] + '/backbone"', first.stdout)
        self.assertIn("Restart your shell to apply changes", first.stderr)
        log = Path(self.env["FIXTURE_LOG"])
        lines = log.read_text().splitlines()
        self.assertIn("tool install --upgrade --prerelease disallow --python >=3.11,<3.14 agent-backbone>=0.4.1", lines)
        self.assertTrue(lines[-1].startswith("-I -B "))
        self.env["FIXTURE_RESUME"] = "3"
        log.write_text("")
        result = self.install(archive)
        lines = log.read_text().splitlines()
        self.assertTrue(any("--resume-pending" in line for line in lines))
        self.assertFalse(any(line.startswith("tool install") for line in lines))
        self.assertNotIn(": failed.", result.stderr)

    def test_stale_shell_path_does_not_block_app_or_provider_setup(self):
        self.backbone_tools()
        self.env["FIXTURE_SHELL_FAIL"] = "1"
        result = self.install(self.package(extra={"agent-interface/setup_backbone.py": "fixture helper"}))
        self.assertTrue(self.link.is_symlink())
        self.assertIn('Backbone command: "' + self.env["FIXTURE_BIN"] + '/backbone"', result.stdout)
        self.assertIn("Shell setup notice (app installation will continue)", result.stderr)
        self.assertIn("Bash configuration files are already up-to-date", result.stderr)
        self.assertIn("Starting and checking Agent Backbone: done.", result.stderr)

    def test_provider_install_path_with_equals_is_executed(self):
        self.backbone_tools()
        self.env["FIXTURE_TOOLS"] = str(self.root / "uv=tools")
        self.install(self.package(extra={"agent-interface/setup_backbone.py": "fixture helper"}))
        self.assertTrue(self.link.is_symlink())

    def test_quiet_setup_reports_elapsed_time_and_cleans_up_progress(self):
        self.backbone_tools()
        self.env["FIXTURE_SETUP_DELAY"] = "6"
        result = self.install(self.package(extra={"agent-interface/setup_backbone.py": "fixture helper"}))
        self.assertIn("Starting and checking Agent Backbone: still running (5s elapsed)", result.stderr)
        self.assertIn("Starting and checking Agent Backbone: done.", result.stderr)
        # install() has a bounded timeout: an orphan heartbeat would otherwise
        # retain its capture pipe indefinitely. It must emit no later updates.
        self.assertTrue(result.stderr.rstrip().endswith("Starting and checking Agent Backbone: done."))

    def test_setup_failure_reports_failed_step_and_preserves_old_app(self):
        self.install(self.package("0.1.1"))
        self.backbone_tools()
        self.env.update(FIXTURE_SHELL_FAIL="1", FIXTURE_SETUP_FAIL="1")
        result = self.install(self.package(extra={"agent-interface/setup_backbone.py": "fixture helper"}), success=False)
        self.assertIn("Starting and checking Agent Backbone: failed.", result.stderr)
        self.assertIn("0.1.1", subprocess.check_output([self.link, "--version"], text=True))

    def test_provider_failure_keeps_previous_app(self):
        self.install(self.package("0.1.1"))
        self.backbone_tools()
        archive = self.package("0.2.0", extra={"agent-interface/setup_backbone.py": "fixture helper"})
        self.env["FIXTURE_INSTALL_FAIL"] = "1"
        self.install(archive, success=False)
        self.assertIn("0.1.1", subprocess.check_output([self.link, "--version"], text=True))
        self.env["FIXTURE_INSTALL_FAIL"] = "0"
        self.env["FIXTURE_SETUP_FAIL"] = "1"
        self.install(archive, success=False)
        self.assertIn("0.1.1", subprocess.check_output([self.link, "--version"], text=True))
        self.env["FIXTURE_RESUME"] = "1"
        Path(self.env["FIXTURE_LOG"]).write_text("")
        self.install(archive, success=False)
        self.assertNotIn("tool install", Path(self.env["FIXTURE_LOG"]).read_text())


if __name__ == "__main__":
    unittest.main()
