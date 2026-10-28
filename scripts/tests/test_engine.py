"""Engine inventory and initialization transactions with inert private fixtures."""

import hashlib
import json
import os
from pathlib import Path
import stat
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from engine_payload import digest, verify
from install_engine import install, initialize, parse_result


class EngineTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.home = self.root / "home with spaces"
        self.home.mkdir()
        self.data = self.home / "provider"
        self.target = "aarch64-apple-darwin"
        self.engine_root = self.home / ".local/lib/agent-interface-engine"

    def payload(self, name="engine-a", behavior="fresh"):
        root = self.root / name
        root.mkdir()
        files = {
            "bin/backbone": (f"#!{sys.executable}\n" + '''
import json, pathlib, sys
request = json.loads(pathlib.Path(sys.argv[3]).read_text())
data = pathlib.Path(request['data_dir']); data.mkdir(exist_ok=True, parents=True)
calls = data / 'calls.jsonl'
with calls.open('a') as out: out.write(json.dumps(request) + '\\n')
''' + ("if len(calls.read_text().splitlines()) == 1: sys.exit(7)\n" if behavior == "interrupt" else "") + '''
result = {'schema_version':1, 'request_id':request['request_id'], 'seq':1,
 'type':'result', 'action':'initialize', 'status':'completed',
 'code':CODE, 'data':{'data_dir':str(data), 'installation':INSTALLATION, 'health':'healthy'}}
print(json.dumps(result))
'''.replace("CODE", repr("existing_preserved" if behavior == "existing" else "initialized"))
                .replace("INSTALLATION", repr("existing_preserved" if behavior == "existing" else "created"))),
            "python/bin/python3": "fixture python, never executed",
            "libexec/tmux": "fixture tmux, never executed",
            "licenses/NOTICE": "fixture notice",
            "engine-license-inventory.json": "{}\n",
        }
        for name, text in files.items():
            path = root / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(text)
            path.chmod(0o755 if name in {"bin/backbone", "python/bin/python3", "libexec/tmux"} else 0o644)
        self.inventory(root)
        return root

    def inventory(self, root):
        rows = []
        for path in sorted(root.rglob("*")):
            if path.name in {"engine-manifest.json", "engine-file-inventory-v1.json"}:
                continue
            row = {"path": path.relative_to(root).as_posix(), "mode": stat.S_IMODE(path.stat().st_mode),
                   "kind": "directory" if path.is_dir() else "file"}
            if path.is_file():
                row.update(sha256=digest(path), size=path.stat().st_size)
            rows.append(row)
        (root / "engine-file-inventory-v1.json").write_text(json.dumps(rows))
        manifest = {
            "schema_version": 1, "payload_id": root.name, "target": self.target,
            "entrypoint": "bin/backbone", "protocols": {"runtime_discovery": 1, "setup": 1},
            "release_ready": False,
            "tmux": {"path": "libexec/tmux", "sha256": digest(root / "libexec/tmux")},
            "files": {"path": "engine-file-inventory-v1.json", "sha256": digest(root / "engine-file-inventory-v1.json")},
            "licenses": {"path": "engine-license-inventory.json", "sha256": digest(root / "engine-license-inventory.json")},
        }
        (root / "engine-manifest.json").write_text(json.dumps(manifest))

    def run_install(self, root):
        with patch.dict(os.environ, {"HOME": str(self.home)}):
            return install(root, self.target, self.home, self.data, "none")

    def test_inventory_requires_explicit_private_opt_in_and_exact_contents(self):
        root = self.payload()
        with self.assertRaisesRegex(ValueError, "release-ready"):
            verify(root, self.target)
        verify(root, self.target, True)
        extra = root / "unlisted.pyc"
        extra.write_bytes(b"cache")
        with self.assertRaisesRegex(ValueError, "exactly"):
            verify(root, self.target, True)
        extra.unlink()
        (root / "libexec/tmux").write_text("changed")
        with self.assertRaisesRegex(ValueError, "content mismatch"):
            verify(root, self.target, True)

    def test_inventory_rejects_symlinks_special_files_and_wrong_target(self):
        root = self.payload()
        with self.assertRaisesRegex(ValueError, "target"):
            verify(root, "x86_64-unknown-linux-gnu", True)
        extra = root / "extra"
        extra.symlink_to(root / "libexec")
        with self.assertRaisesRegex(ValueError, "links"):
            verify(root, self.target, True)
        extra.unlink()
        os.mkfifo(extra)
        with self.assertRaisesRegex(ValueError, "special"):
            verify(root, self.target, True)

    def test_fresh_activation_uses_final_launcher_and_replay_preserves_request(self):
        root = self.payload()
        self.run_install(root)
        active = json.loads((self.engine_root / "active.json").read_text())
        self.assertEqual(active["executable"], str(self.engine_root / root.name / "bin/backbone"))
        self.assertEqual(active["data_dir"], str(self.data))
        self.run_install(root)
        calls = [json.loads(x) for x in (self.data / "calls.jsonl").read_text().splitlines()]
        self.assertEqual(calls[0], calls[1])
        record = next(self.engine_root.glob("initialize-*.json"))
        self.assertEqual(stat.S_IMODE(record.stat().st_mode), 0o600)

    def test_update_preserves_completed_service_choice(self):
        root = self.payload()
        self.run_install(root)
        active = (self.engine_root / "active.json").read_bytes()
        newer = self.payload("engine-b", "existing")
        install(newer, self.target, self.home, self.data, "user")
        calls = [json.loads(x) for x in (self.data / "calls.jsonl").read_text().splitlines()]
        self.assertEqual(calls[0], calls[1])
        self.assertEqual(calls[1]["service"], "none")
        self.assertEqual((self.engine_root / "active.json").read_bytes(), active)

    def test_confirmed_preflight_failure_allows_new_choices_and_keeps_receipt(self):
        root = self.payload()
        failed = {"status": "failed", "retry": "new_request", "completed_steps": ["preflight"]}
        with patch("install_engine.initialize", return_value=failed) as initialize:
            with self.assertRaisesRegex(ValueError, "new installation choices"):
                self.run_install(root)
            first = initialize.call_args.args[1]
        newer = self.payload("engine-b")
        install(newer, self.target, self.home, self.data, "user")
        second = json.loads((self.data / "calls.jsonl").read_text())
        self.assertNotEqual(first["request_id"], second["request_id"])
        self.assertEqual(second["service"], "user")
        self.assertEqual(len(list(self.engine_root.glob("previous-initialize-*.result.json"))), 1)
        active = json.loads((self.engine_root / "active.json").read_text())
        self.assertEqual(active["payload_id"], newer.name)

    def test_mutating_failure_or_unknown_outcome_never_allows_new_choices(self):
        root = self.payload()
        for steps, retry in [(["preflight", "credentials"], "new_request"),
                             (["preflight"], "same_request")]:
            failed = {"status": "failed", "retry": retry, "completed_steps": steps}
            with patch("install_engine.initialize", return_value=failed):
                with self.assertRaisesRegex(ValueError, "saved request"):
                    self.run_install(root)
            with self.assertRaisesRegex(ValueError, "choices differ"):
                install(root, self.target, self.home, self.data, "user")
        with patch("install_engine.initialize", side_effect=ValueError("unknown")):
            with self.assertRaisesRegex(ValueError, "unknown"):
                self.run_install(root)
        with self.assertRaisesRegex(ValueError, "choices differ"):
            install(root, self.target, self.home, self.data, "user")

    def test_interruption_retains_engine_and_replays_old_launcher_with_new_archive(self):
        root = self.payload(behavior="interrupt")
        with self.assertRaisesRegex(ValueError, "final"):
            self.run_install(root)
        self.assertFalse((self.engine_root / "active.json").exists())
        self.assertTrue((self.engine_root / root.name / "bin/backbone").exists())
        newer = self.payload("engine-b", "existing")
        self.run_install(newer)
        active = json.loads((self.engine_root / "active.json").read_text())
        self.assertEqual(active["payload_id"], root.name)
        calls = [json.loads(x) for x in (self.data / "calls.jsonl").read_text().splitlines()]
        self.assertEqual(calls[0], calls[1])

    def test_existing_install_does_not_create_or_replace_activation(self):
        root = self.payload(behavior="existing")
        self.run_install(root)
        active = self.engine_root / "active.json"
        self.assertFalse(active.exists())
        active.write_text('{"existing":"leave untouched"}')
        self.run_install(root)
        self.assertEqual(active.read_text(), '{"existing":"leave untouched"}')

    def test_changed_installed_bytes_are_never_executed(self):
        root = self.payload()
        self.run_install(root)
        executable = self.engine_root / root.name / "bin/backbone"
        executable.write_text("#!/bin/sh\nexit 0\n")
        with self.assertRaisesRegex(ValueError, "content mismatch"):
            self.run_install(root)

    def test_output_requires_matching_request_order_and_completion(self):
        request = {"request_id": "one", "data_dir": "/private/fixture", "service": "none"}
        event = {"schema_version": 1, "request_id": "one", "seq": 1, "type": "result",
                 "action": "initialize", "status": "completed", "code": "initialized",
                 "data": {"data_dir": "/private/fixture", "installation": "created"}}
        data = json.dumps(event).encode() + b"\n"
        parse_result(data, request, 0)
        for output, code in [(data, 2), (data + data, 0), (b"", 0),
                             (data.replace(b'"one"', b'"two"'), 0)]:
            with self.assertRaises(ValueError):
                parse_result(output, request, code)

    def test_timeout_stops_child_and_keeps_unknown_outcome(self):
        executable = self.root / "sleeper"
        executable.write_text(f"#!{sys.executable}\nimport time\ntime.sleep(30)\n")
        executable.chmod(0o755)
        with self.assertRaisesRegex(ValueError, "unknown"):
            initialize(executable, {"request_id": "timeout"}, os.environ["PATH"], timeout=0.01)


if __name__ == "__main__":
    unittest.main()
