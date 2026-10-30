"""Upgrade safety with inert endpoints; no real provider or tmux processes."""

import importlib.util
from importlib.metadata import PackageNotFoundError
import hashlib
import json
from pathlib import Path
import socket
import tempfile
import threading
import unittest
from unittest.mock import patch

SPEC = importlib.util.spec_from_file_location("upgrade_backbone", Path(__file__).parents[1] / "upgrade_backbone.py")
upgrade = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(upgrade)


class FirstSetupTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.home = Path(temporary.name)
        self.data = self.home / ".local/share/agent-backbone"
        self.executable = self.home / "bin/backbone"
        self.root = self.home / ".local/state/agent-interface/backbone"
        self.root.mkdir(parents=True, mode=0o700)
        self.identity = hashlib.sha256(str(self.data).encode()).hexdigest()
        for target, name, kwargs in (
            (upgrade, "package_version", {"return_value": "0.4.3"}),
            (upgrade.setup, "command", {"return_value": b"MainPID=0\nLoadState=not-found\nActiveState=inactive\n"}),
        ):
            mock = patch.object(target, name, **kwargs)
            mock.start()
            self.addCleanup(mock.stop)

    def eligible(self):
        return upgrade.first_setup_without_service(self.home, self.data, self.executable)

    def test_no_service_can_continue_fixed_setup_without_upgrade(self):
        self.assertTrue(self.eligible())
        with patch.object(upgrade, "owned_state") as owned:
            self.assertEqual(upgrade.guard(self.home, self.data, self.executable, "before"), "first_setup")
            owned.assert_not_called()

    def test_saved_request_before_side_effects_is_retained(self):
        request = self.root / (self.identity + ".json")
        record = {"version": "0.4.3", "executable": str(self.executable),
                  "request": {"data_dir": str(self.data), "action": "initialize"}}
        upgrade.setup.save_new(request, record)
        before = request.read_bytes()
        self.assertTrue(self.eligible())
        self.assertEqual(request.read_bytes(), before)
        for key, value in (("version", "0.4.2"), ("executable", "/different/backbone"),
                           ("request", {"data_dir": str(self.data), "action": "first_agent"})):
            with self.subTest(key=key):
                request.write_text(json.dumps(dict(record, **{key: value})))
                self.assertFalse(self.eligible())
        request.write_bytes(before)
        request.chmod(0o644)
        with self.assertRaises(ValueError):
            self.eligible()

    def test_old_provider_or_unknown_manager_is_not_first_setup(self):
        with patch.object(upgrade, "package_version", return_value="0.4.2"):
            self.assertFalse(self.eligible())
        for properties in (b"", b"MainPID=123\nLoadState=loaded\nActiveState=active\n"):
            with patch.object(upgrade.setup, "command", return_value=properties):
                self.assertFalse(self.eligible())

    def test_any_prior_data_unit_outcome_or_hold_defers(self):
        unit = self.home / ".config/systemd/user/agent-backbone.service"
        for path in (self.data, unit, self.root / (self.identity + ".result.json"),
                     self.root / (self.identity + ".upgrade.json")):
            with self.subTest(path=path.name):
                path.parent.mkdir(parents=True, exist_ok=True)
                path.touch()
                self.assertFalse(self.eligible())
                path.unlink()
                path.symlink_to(self.home / "missing")
                self.assertFalse(self.eligible())
                path.unlink()

    def test_cli_redacts_provider_errors_and_preserves_control_exits(self):
        args = ["--executable", str(self.executable), "--phase", "before"]
        for error in (ImportError("private detail"), PackageNotFoundError("private detail"), RuntimeError("private detail")):
            with patch.object(upgrade, "guard", side_effect=error):
                with self.assertRaises(SystemExit) as raised:
                    upgrade.main(args)
                self.assertEqual(str(raised.exception), "Agent Interface: " + upgrade.DEFERRED)
        with patch.object(upgrade, "guard", return_value="first_setup"):
            with self.assertRaises(SystemExit) as raised:
                upgrade.main(args)
            self.assertEqual(raised.exception.code, 3)
        with patch.object(upgrade, "guard", side_effect=KeyboardInterrupt):
            with self.assertRaises(KeyboardInterrupt):
                upgrade.main(args)


class UpgradeTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.path = self.root / "operation.json"
        self.state = {"socket": str(self.root / "private.sock"), "installation_identity": ["fixture", 1, 2], "service_identity": "unit", "main_pid": 123}
        self.health = {"healthy": True, "version": "0.4.3", "started": 1234.5}
        self.process = [123, "1000"]
        self.holds = {"another-operator"}
        self.calls = []
        self.lose_reply = False
        for name, kwargs in [
            ("owned_state", {"side_effect": lambda *_: (self.path, dict(self.state))}),
            ("package_version", {"return_value": "0.4.3"}),
            ("operator_token", {"return_value": "fixture-operator"}),
            ("exchange", {"side_effect": self.exchange}),
        ]:
            mock = patch.object(upgrade, name, **kwargs)
            mock.start()
            self.addCleanup(mock.stop)
        mock = patch.object(upgrade.setup, "check_schema")
        mock.start()
        self.addCleanup(mock.stop)

    def exchange(self, path, method, route, token="", payload=None, expected_process=None):
        self.calls.append((method, route, payload))
        if expected_process is not None and expected_process != self.process:
            raise ValueError("process changed")
        if method == "GET":
            return dict(self.health), list(self.process)
        self.assertTrue(self.path.exists(), "operation must be durable before request")
        operation = payload["operation_id"]
        if payload["enabled"]:
            self.holds.add(operation)
        else:
            self.holds.discard(operation)
        if self.lose_reply:
            raise OSError("lost acknowledgement")
        return {"operation_held": operation in self.holds, "held": bool(self.holds)}, list(self.process)

    def guard(self, phase="before"):
        upgrade.guard(self.root, self.root, self.root / "backbone", phase)

    def test_affected_or_unknown_runtime_never_acquires_hold(self):
        for version in ("0.4.0", "0.4.1", "0.4.2", "0.4.3rc1", None, "unknown"):
            with self.subTest(version=version):
                self.health["version"] = version
                with self.assertRaisesRegex(ValueError, "deferred"):
                    self.guard()
                self.assertFalse(self.path.exists())
                self.assertEqual(self.holds, {"another-operator"})
        self.assertTrue(all(call[0] == "GET" for call in self.calls))

    def test_fixed_launch_update_releases_only_its_own_hold(self):
        self.guard()
        saved = self.path.read_bytes()
        self.assertEqual(len(self.holds), 2)
        self.guard()  # Idempotent retry uses the exact saved operation.
        self.assertEqual(self.path.read_bytes(), saved)
        self.guard("after")
        self.assertEqual(self.holds, {"another-operator"})
        self.assertTrue(json.loads(self.path.read_text())["result"]["released"])

    def test_lost_acquisition_reply_retains_same_operation(self):
        self.lose_reply = True
        with self.assertRaises(OSError):
            self.guard()
        saved = self.path.read_bytes()
        self.lose_reply = False
        self.guard()
        self.assertEqual(self.path.read_bytes(), saved)
        self.assertEqual(len(self.holds), 2)

    def test_changed_process_or_reexec_does_not_release_hold(self):
        self.guard()
        saved = self.path.read_bytes()
        for change in ("process", "started", "service_identity", "socket"):
            with self.subTest(change=change):
                process, health, state = list(self.process), dict(self.health), dict(self.state)
                if change == "process":
                    self.process[0] += 1
                    self.state["main_pid"] += 1
                elif change == "started":
                    self.health["started"] += 1
                else:
                    self.state[change] += "changed"
                with self.assertRaisesRegex(ValueError, "ownership changed"):
                    self.guard("after")
                self.assertEqual(self.path.read_bytes(), saved)
                self.assertEqual(len(self.holds), 2)
                self.process, self.health, self.state = process, health, state

    def test_new_cli_with_old_daemon_is_not_fixed_launch(self):
        self.health["version"] = "0.4.2"
        with self.assertRaisesRegex(ValueError, "deferred"):
            self.guard()
        self.assertFalse(self.path.exists())

    def test_schema_failure_after_package_mutation_keeps_hold(self):
        self.guard()
        with patch.object(upgrade.setup, "check_schema", side_effect=ValueError("incompatible")):
            with self.assertRaisesRegex(ValueError, "incompatible"):
                self.guard("after")
        self.assertEqual(len(self.holds), 2)
        self.assertFalse(json.loads(self.path.read_text())["result"]["released"])

    def test_final_verification_requires_same_process_and_expected_version(self):
        self.guard()
        self.guard("after")
        self.health.update(version="0.4.4", started=1235.5)
        with patch.object(upgrade, "package_version", return_value="0.4.4"):
            self.guard("verify")
            self.health["version"] = "0.4.3"
            with self.assertRaisesRegex(ValueError, "identity or version"):
                self.guard("verify")
            self.health["version"] = "0.4.4"
            self.process[0] += 1
            self.state["main_pid"] += 1
            with self.assertRaisesRegex(ValueError, "identity or version"):
                self.guard("verify")


class TransportTests(unittest.TestCase):
    def test_private_endpoint_sends_hold_and_rejects_changed_peer_before_auth(self):
        # Short path also works on macOS; Linux kernel identity is mocked here.
        with tempfile.TemporaryDirectory(prefix="ai-hold-", dir="/tmp") as temporary:
            root = Path(temporary)
            root.chmod(0o700)
            path = root / "api.sock"
            with socket.socket(socket.AF_UNIX) as server:
                server.bind(str(path))
                path.chmod(0o600)
                server.listen(1)
                server.settimeout(5)
                received = []

                def answer():
                    with server.accept()[0] as client:
                        request = client.recv(4096)
                        received.append(request)
                        if request:
                            client.sendall(b'HTTP/1.1 200 OK\r\n\r\n{"held":true,"operation_held":true}')

                worker = threading.Thread(target=answer)
                worker.start()
                with patch.object(upgrade, "peer_process", return_value=[123, "1"]):
                    response, _ = upgrade.exchange(path, "POST", "/api/upgrade/hold", "fixture", {"enabled": True}, [123, "1"])
                worker.join(5)
                self.assertFalse(worker.is_alive())
                self.assertTrue(response["operation_held"])
                self.assertIn(b'Authorization: Bearer fixture\r\n', received[0])
                worker = threading.Thread(target=answer)
                worker.start()
                with patch.object(upgrade, "peer_process", return_value=[124, "2"]):
                    with self.assertRaisesRegex(ValueError, "process changed"):
                        upgrade.exchange(path, "POST", "/api/upgrade/hold", "fixture", {}, [123, "1"])
                worker.join(5)
                self.assertFalse(worker.is_alive())
                self.assertEqual(received[-1], b"")


if __name__ == "__main__":
    unittest.main()
