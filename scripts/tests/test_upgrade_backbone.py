"""Upgrade safety with inert endpoints; no real provider or tmux processes."""

import importlib.util
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
