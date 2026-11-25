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

ROOT = Path(__file__).resolve().parents[2]
INSTALL = ROOT / "install.sh"
APP_ID = "io.github.eandualem.agent-interface"
BACKBONE_FIXTURE = []


def backbone_fixture():
    """The app's Backbone commands with a test release key, built once."""
    if not BACKBONE_FIXTURE:
        build = subprocess.run(
            ["cargo", "build", "--locked", "-p", "agent-interface-install", "--example", "backbone-fixture",
             "--message-format=json-render-diagnostics"],
            cwd=ROOT, check=True, stdout=subprocess.PIPE, text=True)
        for line in build.stdout.splitlines():
            message = json.loads(line)
            if message.get("reason") == "compiler-artifact" and message["target"]["name"] == "backbone-fixture":
                BACKBONE_FIXTURE.append(message["executable"])
    return BACKBONE_FIXTURE[0]


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
        # Ordinary fixtures must never inspect or configure the host's sandbox
        # policy. Sandbox cases opt into their own redirected paths below.
        self.installer = self.root / "install.sh"
        self.installer.write_text(INSTALL.read_text().replace(
            "/proc/sys/kernel/apparmor_restrict_unprivileged_userns",
            str(self.root / "no-host-userns-policy")))
        self.command("sudo", "exit 99")
        self.command("apt-get", "exit 99")
        self.command("apparmor_parser", "exit 99")
        self.destination = self.home / ".local/lib/agent-interface"
        self.launcher = self.home / f".local/share/applications/{APP_ID}.desktop"
        self.link = self.home / ".local/bin/agent-interface"

    def command(self, name, body):
        path = self.tools / name
        path.write_text("#!/bin/sh\n" + body + "\n")
        path.chmod(0o755)

    def package(self, version="0.0.0", target="x86_64-unknown-linux-gnu", extra=None):
        archive = self.root / "app.tar.gz"
        mac = target.endswith("apple-darwin")
        name = "agent-interface"
        root = "Agent Interface.app/Contents" if mac else name
        resources = root + "/Resources" if mac else root
        executable = root + ("/MacOS" if mac else "/bin") + "/" + name
        files = {resources + "/INSTALLER": APP_ID + "\n",
                 resources + "/TARGET": target + "\n",
                 # Without a fixture, the Backbone commands answer as a
                 # shipped build with no release key does.
                 executable: "#!/bin/sh\n[ \"$1\" != backbone-install ] || {\n"
                             '    [ -z "${TEST_BACKBONE_FIXTURE:-}" ] || exec "$TEST_BACKBONE_FIXTURE" "$@"\n'
                             "    echo 'Agent Backbone is not published yet.' >&2; exit 3\n}\n"
                             f"echo '{name} {version}'\n"}
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
        self.installer.write_text(self.installer.read_text().replace(
            "/usr/share/applications", str(self.system_apps)))
        self.env.update(WSL_DISTRO_NAME="Ubuntu", DISPLAY=":0",
                        TEST_SYSTEM_ACTIONS=str(self.root / "system-actions"))
        self.command("dpkg-query", "exit 1")
        self.command("apt-get", "exit 99")
        self.command("sudo", '''
printf '%s\\n' "$*" >> "$TEST_SYSTEM_ACTIONS"
case "$1" in
    -v) exit 0;;
    apt-get)
        # With TEST_TICKS, the refresh ends once run_step has printed an update.
        if [ "$2" = update ] && [ -n "${TEST_TICKS:-}" ]; then
            i=0
            until [ -e "$TEST_TICKS.second" ]; do
                i=$((i + 1)); [ "$i" -le 400 ] || exit 98
                /bin/sleep 0.05
            done
        fi
        [ "$2" != update ] || exit "${TEST_APT_STATUS:-0}"
        exit 0;;
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
        (policy / "profiles").write_text("")
        restriction = self.root / "restrict-userns"
        restriction.write_text("1\n")
        source = self.root / "distribution-policy"
        source.write_text("# inert distribution policy fixture\n")
        parser = self.tools / "apparmor_parser"
        self.command("apparmor_parser", 'exit "${TEST_PARSER_STATUS:-0}"')
        self.command("timeout", "exit 1")
        self.sandbox_profile = policy / "bwrap-userns-restrict"
        self.installer = self.root / "install-sandbox.sh"
        self.installer.write_text(INSTALL.read_text()
            .replace("/proc/sys/kernel/apparmor_restrict_unprivileged_userns", str(restriction))
            .replace("/sys/kernel/security/apparmor", str(policy))
            .replace("/etc/apparmor.d", str(policy))
            .replace("/usr/share/apparmor/extra-profiles/bwrap-userns-restrict", str(source))
            .replace("/usr/sbin/apparmor_parser", str(parser)))
        self.env["TEST_SYSTEM_ACTIONS"] = str(self.root / "sandbox-actions")
        self.command("sudo", """
printf '%s\\n' "$*" >> "$TEST_SYSTEM_ACTIONS"
[ "$1" != -v ] || exit "${TEST_SUDO_STATUS:-0}"
exec "$@"
""")
        return self.package()

    def test_distribution_sandbox_policy_is_loaded(self):
        self.install(self.sandbox_fixture())
        self.assertEqual(self.sandbox_profile.read_text(), "# inert distribution policy fixture\n")
        actions = Path(self.env["TEST_SYSTEM_ACTIONS"]).read_text()
        self.assertIn("apparmor_parser -r", actions)
        self.assertNotIn("sysctl", actions)

    def test_sandbox_custom_policy_is_preserved(self):
        archive = self.sandbox_fixture()
        self.sandbox_profile.write_text("custom policy\n")
        result = self.install(archive)
        self.assertEqual(self.sandbox_profile.read_text(), "custom policy\n")
        self.assertIn("Existing Bubblewrap policy preserved", result.stderr)
        self.assertTrue(self.destination.exists())

    def test_loaded_sandbox_policy_is_not_replaced(self):
        archive = self.sandbox_fixture()
        (self.sandbox_profile.parent / "profiles").write_text("bwrap (enforce)\n")
        result = self.install(archive)
        self.assertIn("Existing or unreadable sandbox policy preserved", result.stderr)
        self.assertFalse(self.sandbox_profile.exists())
        self.assertNotIn("apparmor_parser", Path(self.env["TEST_SYSTEM_ACTIONS"]).read_text())

    def test_sandbox_setup_without_sudo_still_installs_native_app(self):
        archive = self.sandbox_fixture()
        self.env["TEST_SUDO_STATUS"] = "1"
        result = self.install(archive)
        self.assertIn("native terminals and other views remain available", result.stderr)
        self.assertFalse(self.sandbox_profile.exists())
        self.assertTrue(self.destination.exists())

    def test_sandbox_parser_failure_does_not_persist_policy(self):
        archive = self.sandbox_fixture()
        self.env["TEST_PARSER_STATUS"] = "1"
        result = self.install(archive)
        self.assertIn("Could not load", result.stderr)
        self.assertFalse(self.sandbox_profile.exists())
        self.assertTrue(self.destination.exists())

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

    def heartbeat_ticks(self):
        # run_step's first five-second tick passes at once. Its second tick
        # marks that the first update was printed; the package refresh ends
        # only then, so no wall-clock wait decides the outcome.
        self.env["TEST_TICKS"] = str(self.root / "tick")
        self.command("sleep", '''
if [ "$1" = 5 ] && [ -n "${TEST_TICKS:-}" ]; then
    if [ ! -e "$TEST_TICKS.first" ]; then : > "$TEST_TICKS.first"; exit 0; fi
    : > "$TEST_TICKS.second"
fi
exec /bin/sleep "$@"''')

    def test_slow_package_step_reports_elapsed_time_and_cleans_up_progress(self):
        self.wsl()
        self.heartbeat_ticks()
        result = self.install(self.package())
        step = "Refreshing Ubuntu package information"
        self.assertIn(f"{step}: still running (5s elapsed)", result.stderr)
        self.assertIn(f"{step}: done.", result.stderr)
        # install() has a bounded timeout: an orphan heartbeat would otherwise
        # retain its capture pipe indefinitely. It must emit no later updates.
        self.assertNotIn(f"{step}: still running", result.stderr.split(f"{step}: done.", 1)[1])

    def test_failed_package_step_reports_failure_and_cleans_up_progress(self):
        self.wsl()
        self.heartbeat_ticks()
        self.env["TEST_APT_STATUS"] = "1"
        result = self.install(self.package(), success=False)
        step = "Refreshing Ubuntu package information"
        self.assertIn(f"{step}: still running (5s elapsed)", result.stderr)
        self.assertIn(f"{step}: failed.", result.stderr)
        self.assertIn("Could not refresh Ubuntu package information.", result.stderr)
        self.assertNotIn(f"{step}: still running", result.stderr.split(f"{step}: failed.", 1)[1])

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

    def mock_download(self, archive, prefix="agent-interface"):
        self.env.update(TEST_ARCHIVE=str(archive), TEST_PREFIX=prefix,
                        TEST_DOWNLOADS=str(self.root / "downloads"))
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
    *) exit 22;;
esac
if [ "$write_status" = true ]; then printf 200; fi
''')

    def test_download_installs_the_latest_release(self):
        self.mock_download(self.package("0.1.1"))
        self.install(None)
        self.assertEqual(subprocess.check_output([self.link, "--version"], text=True).strip(), "agent-interface 0.1.1")
        self.assertIn(f"Icon={self.destination}/agent-interface.png\n", self.launcher.read_text())
        self.assertTrue((self.destination / "agent-interface.png").is_file())
        self.assertIn("StartupWMClass=io.github.eandualem.agent-interface\n", self.launcher.read_text())
        self.assertEqual((self.root / "downloads").read_text().splitlines(), [
            "agent-interface-x86_64-unknown-linux-gnu.tar.gz.sha256",
            "agent-interface-x86_64-unknown-linux-gnu.tar.gz"])
        requests = (self.root / "downloads.args").read_text().splitlines()
        self.assertIn("--progress-bar", requests[2])

    def test_checksum_failure_does_not_change_installation(self):
        self.install(self.package("0.1.1"))
        # The release has no checksum: curl fails the fetch, so the archive is
        # never downloaded and the installed app stays.
        self.mock_download(self.package("0.1.0"), prefix="missing")
        self.install(None, success=False)
        self.assertEqual(subprocess.check_output([self.link, "--version"], text=True).strip(), "agent-interface 0.1.1")
        self.assertEqual((self.root / "downloads").read_text().splitlines(), ["agent-interface-x86_64-unknown-linux-gnu.tar.gz.sha256"])

    def test_download_checksum_mismatch_preserves_installation(self):
        self.install(self.package("0.1.1"))
        archive = self.package("0.1.0")
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

    def test_mac_update_and_rollback(self):
        self.command("uname", 'case "$1" in -s) echo Darwin;; -m) echo arm64;; esac')
        self.command("codesign", "exit 0")
        destination = self.home / "Applications/Agent Interface.app"
        binary = destination / "Contents/MacOS/agent-interface"
        self.install(self.package(target="aarch64-apple-darwin"))
        # A failed replacement restores the previous bundle.
        self.command("mv", 'case "$1" in */.agent-interface-stage.*/"Agent Interface.app") exit 1;; esac\n'
                     'exec /bin/mv "$@"')
        self.install(self.package("0.0.1", target="aarch64-apple-darwin"), success=False)
        self.assertEqual(subprocess.check_output([binary, "--version"], text=True).strip(), "agent-interface 0.0.0")
        (self.tools / "mv").unlink()
        self.install(self.package("0.0.1", target="aarch64-apple-darwin"))
        self.assertEqual(subprocess.check_output([binary, "--version"], text=True).strip(), "agent-interface 0.0.1")
        self.install(self.package("0.0.2", target="aarch64-apple-darwin"))
        self.assertEqual(subprocess.check_output([binary, "--version"], text=True).strip(), "agent-interface 0.0.2")

    def test_mac_download_installs_the_latest_release(self):
        self.command("uname", 'case "$1" in -s) echo Darwin;; -m) echo arm64;; esac')
        self.command("codesign", "exit 0")
        self.mock_download(self.package("0.1.1", target="aarch64-apple-darwin"))
        self.install(None)
        binary = self.home / "Applications/Agent Interface.app/Contents/MacOS/agent-interface"
        self.assertEqual(subprocess.check_output([binary, "--version"], text=True).strip(), "agent-interface 0.1.1")
        self.assertEqual((self.root / "downloads").read_text().splitlines(), [
            "agent-interface-aarch64-apple-darwin.tar.gz.sha256",
            "agent-interface-aarch64-apple-darwin.tar.gz"])

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


    def backbone_releases(self):
        """Serve the app and fixture Backbone releases, signed with a test key."""
        self.releases = self.root / "releases"
        self.releases.mkdir()
        self.keys = self.root / "keys"
        self.keys.mkdir()
        fixture = backbone_fixture()
        subprocess.run([fixture, "keygen", str(self.keys)], check=True)
        self.env.update(TEST_BACKBONE_FIXTURE=fixture, BACKBONE_FIXTURE_KEY=(self.keys / "public").read_text(),
                        TEST_ARCHIVE=str(self.package()), TEST_RELEASES=str(self.releases),
                        TEST_DOWNLOADS=str(self.root / "downloads"), TEST_SLEEPS=str(self.root / "sleeps"))
        self.backbone = self.home / ".local/bin/backbone"
        self.command("sleep", 'printf "%s\\n" "$1" >> "$TEST_SLEEPS"')
        stub = self.root / "download.py"
        # Files are served from releases/TAG/NAME. NAME.once, when present,
        # is served in its place once, and NAME.gone makes it missing once,
        # as while the latest release changes. TEST_INTERRUPT interrupts the
        # installer at its first Backbone download.
        stub.write_text('''import os, shutil, signal, sys
from pathlib import Path
args = sys.argv[1:]
url = next(arg for arg in args if arg.startswith('https://'))
if url.endswith('/releases/latest'):
    print('https://github.com/eandualem/agent-interface/releases/tag/vfixture', end='')
    sys.exit(0)
output = Path(args[args.index('-o') + 1])
tag, name = url.split('/releases/download/', 1)[1].split('/')
archive = os.environ['TEST_ARCHIVE']
if tag == 'vfixture':
    shutil.copyfile(archive + '.sha256' if name.endswith('.sha256') else archive, output)
else:
    with open(os.environ['TEST_DOWNLOADS'], 'a') as log:
        log.write(f'{tag}/{name}\\n')
    if os.environ.get('TEST_INTERRUPT'):
        os.kill(os.getppid(), signal.SIGINT)
        sys.exit(22)
    served = Path(os.environ['TEST_RELEASES'], tag, name)
    once = served.with_name(name + '.once')
    gone = served.with_name(name + '.gone')
    if gone.exists():
        gone.unlink()
        print('curl: (22) The requested URL returned error: 404', file=sys.stderr)
        sys.exit(22)
    elif once.exists():
        once.rename(output)
    elif served.exists():
        shutil.copyfile(served, output)
    else:
        print('curl: (22) The requested URL returned error: 404', file=sys.stderr)
        sys.exit(22)
if '--write-out' in args:
    print('200', end='')
''')
        self.command("curl", f"exec {shlex.quote(sys.executable)} {shlex.quote(str(stub))} \"$@\"")

    def sign(self, path):
        subprocess.run([backbone_fixture(), "sign", str(self.keys), str(path)], check=True)

    def publish_backbone(self, version, targets=("x86_64-unknown-linux-gnu",), digest=None):
        """Publish Backbone VERSION and point the latest release at it."""
        release = self.releases / f"backbone-v{version}"
        release.mkdir()
        lines = ["backbone-release-manifest 1", f"version {version}"]
        for target in targets:
            name = f"backbone-{version}-{target}.tar.gz"
            with tarfile.open(release / name, "w:gz") as tar:
                for member, text, mode in (("backbone", f"#!/bin/sh\necho 'backbone {version}'\n", 0o755),
                                           ("LICENSE", "MIT\n", 0o644)):
                    info = tarfile.TarInfo(member)
                    data = text.encode()
                    info.size, info.mode = len(data), mode
                    tar.addfile(info, io.BytesIO(data))
            sha256 = digest or hashlib.sha256((release / name).read_bytes()).hexdigest()
            lines.append(f"artifact {target} {name} {sha256}")
        manifest = release / "backbone-release-manifest"
        manifest.write_text("\n".join(lines) + "\n")
        self.sign(manifest)
        latest = self.releases / "backbone-latest"
        latest.mkdir(exist_ok=True)
        for name in (manifest.name, manifest.name + ".minisig"):
            shutil.copyfile(release / name, latest / name)
        return latest / (manifest.name + ".minisig")

    def bad_signature(self):
        """A signature by the test key of other bytes than the manifest."""
        other = self.root / "other"
        other.write_text("another manifest\n")
        self.sign(other)
        return Path(str(other) + ".minisig").read_bytes()

    def backbone_version(self):
        return subprocess.check_output([self.backbone], text=True).strip()

    def backbone_downloads(self):
        path = self.root / "downloads"
        return path.read_text().splitlines() if path.exists() else []

    def assert_app_installed(self, result):
        self.assertEqual(subprocess.check_output([self.link, "--version"], text=True).strip(), "agent-interface 0.0.0")
        self.assertIn(f"Installed {self.destination}", result.stdout)

    LATEST = ["backbone-latest/backbone-release-manifest", "backbone-latest/backbone-release-manifest.minisig"]

    def test_backbone_clean_install(self):
        self.backbone_releases()
        self.publish_backbone("0.2.0")
        result = self.install(None)
        self.assert_app_installed(result)
        self.assertEqual(self.backbone_version(), "backbone 0.2.0")
        self.assertEqual(self.backbone.stat().st_mode & 0o777, 0o755)
        self.assertIn(f"Installed Agent Backbone 0.2.0 in {self.backbone}", result.stdout)
        self.assertEqual(self.backbone_downloads(), self.LATEST + [
            "backbone-v0.2.0/backbone-0.2.0-x86_64-unknown-linux-gnu.tar.gz"])
        self.assertEqual(sorted(path.name for path in self.backbone.parent.iterdir()),
                         [".agent-interface-backbone", "agent-interface", "backbone"])

    def test_backbone_bad_signature_is_downloaded_again_once(self):
        self.backbone_releases()
        signature = self.publish_backbone("0.2.0")
        signature.with_name(signature.name + ".once").write_bytes(self.bad_signature())
        result = self.install(None)
        self.assertEqual(self.backbone_version(), "backbone 0.2.0")
        self.assertIn("again in 5 seconds", result.stderr)
        self.assertEqual((self.root / "sleeps").read_text(), "5\n")
        self.assertEqual(self.backbone_downloads()[:4], self.LATEST * 2)

    def test_backbone_missing_file_is_downloaded_again_once(self):
        self.backbone_releases()
        signature = self.publish_backbone("0.2.0")
        signature.with_name(signature.name + ".gone").write_text("")
        result = self.install(None)
        self.assertEqual(self.backbone_version(), "backbone 0.2.0")
        self.assertIn("again in 5 seconds", result.stderr)
        self.assertEqual(self.backbone_downloads()[:4], self.LATEST * 2)

    def test_backbone_interrupt_keeps_the_app(self):
        self.backbone_releases()
        self.publish_backbone("0.2.0")
        self.env["TEST_INTERRUPT"] = "1"
        self.install(None, success=False)
        self.assertEqual(subprocess.check_output([self.link, "--version"], text=True).strip(), "agent-interface 0.0.0")
        self.assertEqual(sorted(path.name for path in self.destination.parent.iterdir()),
                         [".agent-interface-download", "agent-interface"])
        self.assertFalse(self.backbone.exists())

    def test_backbone_second_bad_signature_is_refused(self):
        self.backbone_releases()
        self.publish_backbone("0.2.0").write_bytes(self.bad_signature())
        result = self.install(None)
        self.assert_app_installed(result)
        self.assertFalse(self.backbone.exists())
        self.assertIn("signature does not hold", result.stderr)
        self.assertIn("Agent Backbone was not installed. Rerun the installer to try again.", result.stderr)
        self.assertEqual(self.backbone_downloads(), self.LATEST * 2)

    def test_backbone_older_release_is_refused(self):
        self.backbone_releases()
        self.publish_backbone("0.3.0")
        self.install(None)
        self.publish_backbone("0.2.0")
        result = self.install(None)
        self.assertIn("older than 0.3.0", result.stderr)
        self.assertIn("Agent Backbone was not updated. Rerun the installer to try again.", result.stderr)
        self.assertEqual(self.backbone_version(), "backbone 0.3.0")

    def test_backbone_missing_target_and_wrong_digest_are_refused(self):
        self.backbone_releases()
        self.publish_backbone("0.2.0", targets=("aarch64-apple-darwin",))
        result = self.install(None)
        self.assertIn("has nothing for x86_64-unknown-linux-gnu", result.stderr)
        self.assertEqual(self.backbone_downloads(), self.LATEST)
        self.publish_backbone("0.3.0", digest="0" * 64)
        result = self.install(None)
        self.assertIn("digest differs", result.stderr)
        self.assertIn("Rerun the installer", result.stderr)
        self.assertFalse(self.backbone.exists())
        self.assertEqual(sorted(path.name for path in self.backbone.parent.iterdir()), ["agent-interface"])

    def test_backbone_earlier_installation_is_left_in_place(self):
        self.backbone_releases()
        self.publish_backbone("0.2.0")
        self.backbone.parent.mkdir(parents=True)
        self.backbone.write_text("#!/bin/sh\necho earlier\n")
        self.backbone.chmod(0o755)
        service = self.home / ".config/systemd/user/agent-backbone.service"
        for case in ("program", "service"):
            with self.subTest(case=case):
                result = self.install(None)
                self.assert_app_installed(result)
                self.assertIn("earlier Agent Backbone was found and left in place", result.stderr)
                self.assertNotIn("Rerun the installer", result.stderr)
                self.assertEqual(self.backbone_downloads(), [])
            if case == "program":
                self.assertEqual(self.backbone_version(), "earlier")
                # Next, only the earlier service's login service is in the way.
                self.backbone.unlink()
                service.parent.mkdir(parents=True)
                service.write_text("[Unit]\n")
        self.assertEqual(service.read_text(), "[Unit]\n")
        self.assertFalse(self.backbone.exists())

    def test_backbone_rerun_updates(self):
        self.backbone_releases()
        self.publish_backbone("0.2.0")
        self.install(None)
        self.publish_backbone("0.3.0")
        result = self.install(None)
        self.assertIn("Installed Agent Backbone 0.3.0", result.stdout)
        self.assertEqual(self.backbone_version(), "backbone 0.3.0")

    def test_backbone_rerun_after_backbone_upgrade(self):
        self.backbone_releases()
        self.publish_backbone("0.2.0")
        self.install(None)
        # `backbone upgrade` replaces the program in place; it stays this
        # installer's, at the version it reports.
        self.backbone.write_text("#!/bin/sh\necho 'backbone 0.4.0'\n")
        self.publish_backbone("0.3.0")
        result = self.install(None)
        self.assertIn("older than 0.4.0", result.stderr)
        self.assertEqual(self.backbone_version(), "backbone 0.4.0")
        self.publish_backbone("0.5.0")
        self.install(None)
        self.assertEqual(self.backbone_version(), "backbone 0.5.0")

    def test_backbone_shipped_build_without_a_key(self):
        self.backbone_releases()
        self.publish_backbone("0.2.0")
        del self.env["BACKBONE_FIXTURE_KEY"]
        result = self.install(None)
        self.assert_app_installed(result)
        self.assertIn("Agent Backbone is not published yet, so it was not installed.", result.stderr)
        self.assertNotIn("Rerun the installer", result.stderr)
        self.assertFalse(self.backbone.exists())
        self.assertEqual(self.backbone_downloads(), [])


if __name__ == "__main__":
    unittest.main()
