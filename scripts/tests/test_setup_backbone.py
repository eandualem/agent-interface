"""Installer recovery with a fake provider; never start a user service."""
import importlib.util
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

SPEC = importlib.util.spec_from_file_location("setup_backbone", Path(__file__).parents[1] / "setup_backbone.py")
setup = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(setup)


class SetupTests(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.home = Path(directory.name) / "home with spaces"
        self.home.mkdir()
        self.data = self.home / ".local/share/agent-backbone"
        self.executable = self.home / "tools/agent-backbone/bin/backbone"
        self.setting = ""
        self.healthy = False
        self.calls = []
        self.fail_after_config = False
        self.service = "original-service"
        self.existing = False
        for name, implementation in (("check_schema", lambda *_: None),
                                     ("initialize", self.initialize),
                                     ("service_identity", lambda *_: self.service),
                                     ("socket_setting", lambda *_: self.setting),
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
        if arguments[:2] == ["config", "set"]:
            self.setting = arguments[3]
            if self.fail_after_config:
                raise ValueError("lost reply after config commit")
        elif arguments == ["service", "restart"]:
            self.healthy = True
        else:
            self.fail(arguments)
        return b""

    def install(self, version="0.4.0", resume=False):
        return setup.install(self.executable, self.home, self.data, version, "fixture installation", resume)

    def test_fresh_setup_and_repeat_preserve_completed_choices(self):
        self.install()
        self.assertEqual(self.setting, str(self.data / "run/backbone.sock"))
        self.assertEqual(sum(call == ("service", "restart") for call in self.calls), 1)
        self.setting = "a later user choice"
        before = list(self.calls)
        self.install("0.4.1")
        self.assertEqual(self.calls, before)
        self.assertEqual(self.setting, "a later user choice")

    def test_existing_installation_does_not_configure_or_restart(self):
        self.existing = True
        self.install()
        self.assertEqual([call[0] for call in self.calls], ["initialize"])
        self.assertEqual(self.setting, "")

    def test_lost_config_reply_resumes_without_rewriting_and_before_upgrade(self):
        self.fail_after_config = True
        with self.assertRaisesRegex(ValueError, "lost reply"):
            self.install()
        original = self.calls[0][1]
        with self.assertRaisesRegex(ValueError, "installation changed"):
            self.install("0.4.1", resume=True)
        self.fail_after_config = False
        self.install(resume=True)
        self.assertEqual(sum(call[:2] == ("config", "set") for call in self.calls), 1)
        self.assertEqual(self.calls[0][1], original)
        self.assertEqual(sum(call == ("service", "restart") for call in self.calls), 1)

    def test_changed_service_or_socket_is_preserved_on_retry(self):
        self.fail_after_config = True
        with self.assertRaises(ValueError):
            self.install()
        self.fail_after_config = False
        self.service = "replacement-service"
        with self.assertRaisesRegex(ValueError, "service or socket choice changed"):
            self.install(resume=True)
        self.service = "original-service"
        self.setting = "another socket"
        with self.assertRaisesRegex(ValueError, "service or socket choice changed"):
            self.install(resume=True)
        self.assertNotIn(("service", "restart"), self.calls)

    def test_replaced_credentials_are_not_used_for_setup_retry(self):
        self.fail_after_config = True
        with self.assertRaises(ValueError):
            self.install()
        (self.data / ".env").write_text("replacement credential")
        with self.assertRaisesRegex(ValueError, "installation changed"):
            self.install(resume=True)
        self.assertNotIn(("service", "restart"), self.calls)


    def test_completed_receipt_does_not_prevent_clean_reinstall(self):
        import shutil

        self.install()
        original = self.calls[0][1]["request_id"]
        shutil.rmtree(self.data)
        self.setting, self.healthy = "", False
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


class SchemaTests(unittest.TestCase):
    def test_old_provider_is_rejected_before_initialize(self):
        with patch.object(setup, "command", return_value=b'{}'):
            with self.assertRaisesRegex(ValueError, "does not support"):
                setup.check_schema(Path("/fixture/backbone"), {})


class ServiceIdentityTests(unittest.TestCase):
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
