"""Installer recovery with a fake provider; never start a user service."""
import importlib.util
from pathlib import Path
import re
import tempfile
import unittest
from unittest.mock import patch

SPEC = importlib.util.spec_from_file_location("setup_backbone", Path(__file__).parents[1] / "setup_backbone.py")
setup = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(setup)

STABLE_VERSIONS = (
    "0.4.5", "0.4.5.post1", "0.4.5.0", "0.4.10", "0.5",
    "0.4.5+local.1", "1!0.1", "0!0.4.5", "0.4.5+ubuntu-1",
    "0.4.5+ubuntu_1", "0.4.5+Ubuntu-1_build.2", "0.4.5+dev.1",
    "0.4.5-1", "0.4.5rev2", "0.4.5_r_3", "0.4.5.post",
    "0.4.5-post-2", " v00!00.04.005-1+Ubuntu_1-x \n",
)
REJECTED_VERSIONS = (
    "0.4.4.post99", "0.4", "0.4.5rc1", "0.4.5.dev1", "0.5rc1",
    "0.4.5.post1.dev1", "0.4.5-1.dev2", "0.4.5rc1+ubuntu-1",
    "unknown", "0.4.5junk", "0.4.5+", "0.4.5+ubuntu--1",
    "0.4.5+ubuntu_", "0.4.5+_ubuntu", "0.4.5+ubuntu/1", "0.4.5-",
)


class VersionTests(unittest.TestCase):
    def test_installer_checks_versions_without_new_archive_helper_arguments(self):
        installer = (Path(__file__).parents[2] / "install.sh").read_text()
        source = re.search(r"(?s)-c '(\nfrom importlib.metadata import version.*?)\n'", installer)[1]

        def at_profiles_release(value):
            # The installer's floor is the first profiles release, 0.5.0.
            return (value.replace("0.4.4.post99", "0.4.99.post99").replace("0.4.10", "0.5.10")
                    .replace("0.4.5", "0.5.0").replace("00.04.005", "00.05.000"))

        for original in STABLE_VERSIONS + REJECTED_VERSIONS:
            accepted = original in STABLE_VERSIONS
            value = at_profiles_release(original)
            with self.subTest(value=value), patch("importlib.metadata.version", return_value=value):
                with self.assertRaises(SystemExit) as result:
                    exec(source, {})
                self.assertEqual(result.exception.code, 0 if accepted else 1)

    def test_stable_metadata_and_numeric_release_order(self):
        for value in STABLE_VERSIONS:
            with self.subTest(value=value):
                self.assertTrue(setup.stable_version_at_least(value, (0, 4, 5)))
        for value in REJECTED_VERSIONS + (None,):
            with self.subTest(value=value):
                self.assertFalse(setup.stable_version_at_least(value, (0, 4, 5)))
        self.assertTrue(setup.stable_version_at_least("0.4.3.post1", (0, 4, 3)))
        self.assertTrue(setup.stable_version_at_least("0.4.3+ubuntu-1", (0, 4, 3)))
        self.assertFalse(setup.stable_version_at_least("0.4.2.post9", (0, 4, 3)))


class SetupTests(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.home = Path(directory.name) / "home with spaces"
        self.home.mkdir()
        self.data = self.home / ".local/share/agent-backbone"
        self.executable = self.home / "tools/agent-backbone/bin/backbone"
        self.healthy = False
        self.calls = []
        self.fail_restart = False
        self.service = "original-service"
        self.existing = False
        for name, implementation in (("check_schema", lambda *_: None),
                                     ("initialize", self.initialize),
                                     ("service_identity", lambda *_: self.service),
                                     ("socket_healthy", lambda *_: self.healthy),
                                     ("command", self.command)):
            mock = patch.object(setup, name, side_effect=implementation)
            mock.start()
            self.addCleanup(mock.stop)

    def initialize(self, executable, request, path_env):
        self.calls.append(("initialize", request.copy()))
        self.data.mkdir(parents=True, exist_ok=True)
        if not (self.data / ".env").exists():
            (self.data / ".env").write_text("fixture credential\n")
            (self.data / ".env").chmod(0o600)
            (self.data / "backbone.db").write_bytes(b"fixture database")
        return {"status": "completed", "code": "existing_preserved" if self.existing else "initialized"}

    def command(self, executable, arguments, env):
        self.calls.append(tuple(arguments))
        self.assertEqual(env["BACKBONE_DATA_DIR"], str(self.data))
        if arguments == ["service", "restart"]:
            if self.fail_restart:
                raise ValueError("lost reply from restart")
            self.healthy = True
        else:
            self.fail(arguments)
        return b""

    def install(self, version="0.5.0", resume=False):
        return setup.install(self.executable, self.home, self.data, version, "fixture installation", resume)

    def test_fresh_setup_and_repeat_preserve_completed_choices(self):
        self.install()
        self.assertEqual(sum(call == ("service", "restart") for call in self.calls), 1)
        self.assertNotIn("config", [call[0] for call in self.calls])
        before = list(self.calls)
        with patch.object(setup, "socket_version", return_value="0.5.1"):
            self.install("0.5.1")
        self.assertEqual(self.calls, before)

    def test_a_healthy_socket_needs_no_restart(self):
        self.healthy = True
        self.install()
        self.assertEqual([call[0] for call in self.calls], ["initialize"])

    def test_existing_installation_does_not_configure_or_restart(self):
        self.existing = True
        self.install()
        self.assertEqual([call[0] for call in self.calls], ["initialize"])

    def test_upgrade_waits_for_reload_without_replaying_or_restarting(self):
        self.install()
        root = self.home / ".local/state/agent-interface/backbone"
        saved = {path: path.read_bytes() for path in root.glob("*.json")}
        before = list(self.calls)
        with patch.object(setup, "socket_version", side_effect=["0.5.1", None, "0.5.2"]) as health, \
                patch.object(setup.time, "sleep"):
            self.install("0.5.2")
        self.assertEqual(health.call_count, 3)
        self.assertEqual(self.calls, before)
        self.assertEqual(saved, {path: path.read_bytes() for path in root.glob("*.json")})

    def test_upgrade_deadline_and_resume_preserve_saved_installation(self):
        import json

        self.install()
        root = self.home / ".local/state/agent-interface/backbone"
        outcome = next(root.glob("*.result.json"))
        saved = outcome.read_bytes()
        state = json.loads(saved)["result"]
        before = list(self.calls)
        with self.assertRaisesRegex(ValueError, "automatic reload may be disabled or held"):
            setup.wait_for_installed_service(self.executable, self.home, self.data, state, "0.5.2", {}, timeout=0)
        with patch.object(setup, "socket_version") as health:
            self.install("0.5.2", resume=True)
            health.assert_not_called()
        self.assertEqual(self.calls, before)
        self.assertEqual(outcome.read_bytes(), saved)

    def test_upgrade_preserves_replaced_service_management(self):
        self.install()
        before = list(self.calls)
        for error in (FileNotFoundError("removed unit"), ValueError("different executable")):
            with self.subTest(error=type(error).__name__), \
                    patch.object(setup, "service_identity", side_effect=error), \
                    patch.object(setup, "socket_version") as health:
                self.install("0.5.2")
                health.assert_not_called()
        self.assertEqual(self.calls, before)

    def test_lost_restart_reply_resumes_without_replaying_and_before_upgrade(self):
        self.fail_restart = True
        with self.assertRaisesRegex(ValueError, "lost reply"):
            self.install()
        original = self.calls[0][1]
        with self.assertRaisesRegex(ValueError, "installation changed"):
            self.install("0.5.1", resume=True)
        self.fail_restart = False
        self.install(resume=True)
        self.assertEqual([call[0] for call in self.calls].count("initialize"), 1)
        self.assertEqual(self.calls[0][1], original)
        self.assertTrue(self.healthy)

    def test_changed_service_is_preserved_on_retry(self):
        self.fail_restart = True
        with self.assertRaises(ValueError):
            self.install()
        self.fail_restart = False
        restarts = self.calls.count(("service", "restart"))
        self.service = "replacement-service"
        with self.assertRaisesRegex(ValueError, "service changed"):
            self.install(resume=True)
        self.assertEqual(self.calls.count(("service", "restart")), restarts)

    def test_replaced_credentials_are_not_used_for_setup_retry(self):
        self.fail_restart = True
        with self.assertRaises(ValueError):
            self.install()
        restarts = self.calls.count(("service", "restart"))
        (self.data / ".env").write_text("replacement credential")
        with self.assertRaisesRegex(ValueError, "installation changed"):
            self.install(resume=True)
        self.assertEqual(self.calls.count(("service", "restart")), restarts)


    def test_completed_receipt_does_not_prevent_clean_reinstall(self):
        import shutil

        self.install()
        original = self.calls[0][1]["request_id"]
        shutil.rmtree(self.data)
        self.healthy = False
        self.assertFalse(self.install(resume=True)[1])
        self.install()
        requests = [call[1] for call in self.calls if call[0] == "initialize"]
        self.assertEqual(len(requests), 2)
        self.assertNotEqual(requests[-1]["request_id"], original)
        self.assertTrue(self.healthy)

    def test_provider_diagnostic_and_no_retry_are_preserved(self):
        result = {"status": "action_required", "code": "operator_required",
                  "message": "Use the existing installation operator key.",
                  "retry": "none", "completed_steps": ["preflight"]}
        with patch.object(setup, "initialize", return_value=result) as initialize:
            for _ in range(2):
                with self.assertRaisesRegex(ValueError, "operator_required:.*operator key.*Automatic retry is disabled"):
                    self.install()
            self.assertEqual(initialize.call_count, 1)
        result["message"] = "control\x1b[2J"
        self.assertNotIn("\x1b", str(setup.initialization_error(result)))


    def test_a_release_before_profiles_is_refused_before_initialize(self):
        with self.assertRaisesRegex(ValueError, "needs Agent Backbone 0.5.0 or newer, and 0.4.5 is installed"):
            self.install("0.4.5")
        self.assertEqual(self.calls, [])

    def test_a_request_saved_before_profiles_is_set_aside(self):
        root = self.home / ".local/state/agent-interface/backbone"
        self.fail_restart = True
        with self.assertRaises(ValueError):
            self.install()
        import hashlib
        import json
        identity = hashlib.sha256(str(self.data).encode()).hexdigest()
        record = json.loads((root / (identity + ".json")).read_text())
        record["version"] = "0.4.5"
        (root / (identity + ".json")).write_text(json.dumps(record))
        (root / (identity + ".result.json")).unlink()
        self.fail_restart = False
        self.assertEqual(self.install(resume=True), ("No interrupted Backbone setup.", False))
        self.install()
        self.assertEqual([call[0] for call in self.calls].count("initialize"), 2)
        self.assertEqual(len(list(root.glob("previous-*.json"))), 1)

class SchemaTests(unittest.TestCase):
    def test_old_provider_is_rejected_before_initialize(self):
        with patch.object(setup, "command", return_value=b'{}'):
            with self.assertRaisesRegex(ValueError, "does not support"):
                setup.check_schema(Path("/fixture/backbone"), {})


class ServiceIdentityTests(unittest.TestCase):
    def test_version_probe_requires_private_healthy_bounded_json(self):
        import socket
        import threading

        responses = [
            (b'HTTP/1.1 200 OK\r\n\r\n{"healthy":true,"version":"0.4.2"}', "0.4.2"),
            (b'HTTP/1.1 200 OK\r\n\r\n{"healthy":false,"version":"0.4.2"}', None),
            (b'HTTP/1.1 503 Unavailable\r\n\r\n{"healthy":true,"version":"0.4.2"}', None),
            (b'HTTP/1.1 200 OK\r\n\r\n[]', None),
            (b'HTTP/1.1 200 OK\r\n\r\n' + b' ' * 65537, None),
        ]
        with tempfile.TemporaryDirectory(prefix="ai-version-") as temporary:
            directory = Path(temporary)
            directory.chmod(0o700)
            path = directory / "health.sock"
            with socket.socket(socket.AF_UNIX) as server:
                server.bind(str(path))
                path.chmod(0o600)
                server.listen(1)
                server.settimeout(5)
                requests = []
                for response, expected in responses:
                    def answer(response=response):
                        with server.accept()[0] as client:
                            requests.append(client.recv(4096))
                            try:
                                client.sendall(response)
                            except BrokenPipeError:
                                pass  # An oversized response can be rejected early.

                    worker = threading.Thread(target=answer)
                    worker.start()
                    self.assertEqual(setup.socket_version(path), expected)
                    worker.join(timeout=5)
                    self.assertFalse(worker.is_alive())
                self.assertEqual(requests, [b"GET /health HTTP/1.0\r\nHost: localhost\r\nConnection: close\r\n\r\n"] * len(responses))
                def rejected_peer():
                    with server.accept()[0] as client:
                        requests.append(client.recv(4096))

                worker = threading.Thread(target=rejected_peer)
                worker.start()
                with patch.object(setup, "same_user_peer", return_value=False):
                    self.assertIsNone(setup.socket_version(path))
                worker.join(timeout=5)
                self.assertFalse(worker.is_alive())
                self.assertEqual(requests[-1], b"")
                path.chmod(0o666)
                self.assertIsNone(setup.socket_version(path))

    def test_definition_and_loaded_service_must_belong_to_same_installation(self):
        import plistlib

        with tempfile.TemporaryDirectory() as temporary:
            home = Path(temporary) / "home with spaces"
            data = home / "data"
            executable = home / "tools/bin/backbone"
            for system in ("Darwin", "Linux"):
                with self.subTest(system=system), patch("platform.system", return_value=system):
                    if system == "Darwin":
                        path = home / "Library/LaunchAgents/dev.agent-backbone.plist"
                        content = plistlib.dumps({"ProgramArguments": [str(executable), "up"],
                                                  "EnvironmentVariables": {"BACKBONE_DATA_DIR": str(data)}})
                        loaded = f"BACKBONE_DATA_DIR => {data}\n".encode()
                    else:
                        path = home / ".config/systemd/user/agent-backbone.service"
                        content = f'ExecStart="{executable}" up\nEnvironment="BACKBONE_DATA_DIR={data}"\n'.encode()
                        loaded = f'"BACKBONE_DATA_DIR={data}"'.encode()
                    path.parent.mkdir(parents=True)
                    path.write_bytes(content)
                    with patch.object(setup, "command", return_value=loaded):
                        self.assertEqual(len(setup.service_identity(home, data, executable, {})), 64)
                    with patch.object(setup, "command", return_value=loaded.replace(b"/data", b"/other")):
                        with self.assertRaisesRegex(ValueError, "different installation"):
                            setup.service_identity(home, data, executable, {})
                    path.write_bytes(content.replace(b"/data", b"/other"))
                    with patch.object(setup, "command", return_value=loaded):
                        with self.assertRaisesRegex(ValueError, "different installation"):
                            setup.service_identity(home, data, executable, {})

    def test_provider_generated_systemd_unit_with_dollar_and_percent(self):
        # Retained output of Backbone's _unit, including its ExecStart escaping.
        fixture = Path(__file__).parent / "fixtures/backbone-user.service"
        with tempfile.TemporaryDirectory() as temporary:
            home = Path(temporary)
            path = home / ".config/systemd/user/agent-backbone.service"
            path.parent.mkdir(parents=True)
            path.write_bytes(fixture.read_bytes())
            data = Path("/home/fixture $cash%/data")
            executable = Path("/home/fixture $cash%/tools/bin/backbone")
            with patch("platform.system", return_value="Linux"), patch.object(
                    setup, "command", return_value=b'"BACKBONE_DATA_DIR=/home/fixture $cash%/data"'):
                self.assertEqual(len(setup.service_identity(home, data, executable, {})), 64)

    def test_health_probe_uses_private_socket_without_credentials(self):
        import socket
        import threading

        with tempfile.TemporaryDirectory(prefix="ai-socket-") as temporary:
            directory = Path(temporary)
            directory.chmod(0o700)
            path = directory / "health.sock"
            requests = []
            with socket.socket(socket.AF_UNIX) as server:
                server.bind(str(path))
                path.chmod(0o600)
                server.listen(1)
                server.settimeout(5)

                def answer():
                    with server.accept()[0] as client:
                        requests.append(client.recv(4096))
                        client.sendall(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}")

                worker = threading.Thread(target=answer)
                worker.start()
                self.assertTrue(setup.socket_healthy(path))
                worker.join(timeout=5)
                self.assertEqual(requests, [b"GET /health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"])
                path.chmod(0o666)
                self.assertFalse(setup.socket_healthy(path))


class UpgradeCheckTests(unittest.TestCase):
    def test_only_an_installation_without_its_default_profile_is_refused(self):
        import os
        import sqlite3

        installer = (Path(__file__).parents[2] / "install.sh").read_text()
        source = re.search(r"(?s)-c '(\nimport os, sqlite3.*?)\n'", installer)[1]

        def check(home, data):
            with patch.dict(os.environ, {"HOME": str(home), "BACKBONE_DATA_DIR": str(data)}):
                with self.assertRaises(SystemExit) as result:
                    exec(source, {})
            return result.exception.code

        with tempfile.TemporaryDirectory() as directory:
            home = Path(directory).resolve()
            data = home / ".local/share/agent-backbone"
            data.mkdir(parents=True)
            self.assertEqual(check(home, data), 1)  # no registry
            registry = home / ".local/share/agent-backbone-profiles/registry.db"
            registry.parent.mkdir(parents=True)
            with sqlite3.connect(registry) as db:
                db.execute("CREATE TABLE profiles (profile_id TEXT, data_dir TEXT, state TEXT)")
                db.execute("CREATE TABLE default_profiles (profile_id TEXT)")
                db.execute("CREATE TABLE profile_bindings (profile_id TEXT)")
                db.execute("INSERT INTO profiles VALUES ('p', ?, 'creating')", (str(data),))
                db.execute("INSERT INTO default_profiles VALUES ('p')")
            self.assertEqual(check(home, data), 1)  # Default without a binding is not ready
            with sqlite3.connect(registry) as db:
                db.execute("INSERT INTO profile_bindings VALUES ('p')")
            self.assertEqual(check(home, data), 0)
            link = home / "linked-data"
            link.symlink_to(data)
            self.assertEqual(check(home, link), 0)
            with sqlite3.connect(registry) as db:
                db.execute("UPDATE profiles SET state = 'failed'")
            self.assertEqual(check(home, data), 1)
            registry.write_bytes(b"not a database")
            self.assertEqual(check(home, data), 2)


class InitProfilesTests(unittest.TestCase):
    def setUp(self):
        import os
        import socket
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.root = Path(directory.name).resolve()
        self.data = self.root / "agent-backbone"
        (self.data / "run").mkdir(parents=True)
        (self.data / "backbone.db").write_bytes(b"fixture database")
        self.executable = self.root / "backbone"
        probe = socket.socket()
        probe.bind(("127.0.0.1", 0))
        self.free = probe.getsockname()[1]
        probe.close()
        self.env = dict(os.environ, BACKBONE_DATA_DIR=str(self.data), BACKBONE_PORT=str(self.free))

    def fake(self, body):
        self.executable.write_text("#!/bin/sh\n" + body + "\n")
        self.executable.chmod(0o755)

    def test_runs_init_and_prints_what_it_reported(self):
        import io
        from contextlib import redirect_stdout
        self.fake('[ "$1" = init ] && echo "profile ready: Default (abc123)"')
        printed = io.StringIO()
        with redirect_stdout(printed):
            setup.init_profiles(self.executable, self.data, self.env)
        self.assertIn("profile ready: Default (abc123)", printed.getvalue())
        self.assertEqual(sorted(p.name for p in self.root.iterdir()), ["agent-backbone", "backbone"])

    def test_failed_init_says_it_was_not_started_and_what_to_run(self):
        self.fake('echo "error: The installation has no identity yet \033[2J"; exit 1')
        with self.assertRaisesRegex(ValueError, "not started again.*no identity yet.*run `backbone init`") as caught:
            setup.init_profiles(self.executable, self.data, self.env)
        self.assertNotIn("\x1b", str(caught.exception))

    def test_anything_still_answering_stops_the_upgrade(self):
        import socket
        self.fake('touch "$0.ran"')
        listener = socket.socket(socket.AF_UNIX)
        self.addCleanup(listener.close)
        listener.bind(str(self.data / "run/backbone.sock"))
        listener.listen()
        with patch.object(setup.time, "sleep"):
            with self.assertRaisesRegex(ValueError, "still answering on .*backbone.sock.*Nothing else was changed"):
                setup.init_profiles(self.executable, self.data, self.env)
        listener.close()
        (self.data / "run/backbone.sock").unlink()
        port = socket.socket()
        self.addCleanup(port.close)
        port.bind(("127.0.0.1", 0))
        port.listen()
        (self.data / ".env").write_text(f"BACKBONE_PORT={port.getsockname()[1]}\n")
        del self.env["BACKBONE_PORT"]
        with patch.object(setup.time, "sleep"):
            with self.assertRaisesRegex(ValueError, f"still answering on port {port.getsockname()[1]}"):
                setup.ensure_stopped(self.data, self.env)
        self.assertFalse(Path(str(self.executable) + ".ran").exists())
