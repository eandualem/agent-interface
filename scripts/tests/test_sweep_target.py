"""Check which artifacts the sweep keeps."""

import importlib.util
import unittest
from pathlib import Path

SPEC = importlib.util.spec_from_file_location(
    "sweep_target", Path(__file__).resolve().parents[1] / "sweep_target.py"
)
sweep = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(sweep)


class SweepTests(unittest.TestCase):
    def test_keeps_only_units_the_latest_build_reported(self):
        artifact = {
            "reason": "compiler-artifact",
            "filenames": ["/w/target/debug/deps/libfoo-0123456789abcdef.rlib"],
        }
        script = {
            "reason": "build-script-executed",
            "out_dir": "/w/target/debug/build/bar-fedcba9876543210/out",
        }
        hashes = sweep.unit_hashes(artifact) | sweep.unit_hashes(script)
        entries = [
            "libfoo-0123456789abcdef.rlib",
            "foo-0123456789abcdef.d",
            "foo-0123456789abcdef.foo.1a2b-cgu.0.rcgu.o",
            "bar-fedcba9876543210",
            "libfoo-1111111111111111.rlib",
            "bar-2222222222222222",
            "no-hash-here",
        ]
        self.assertEqual(
            list(sweep.unused(entries, hashes, set())),
            ["libfoo-1111111111111111.rlib", "bar-2222222222222222"],
        )

    def test_keeps_a_binary_copied_out_without_its_hash(self):
        binary = {
            "reason": "compiler-artifact",
            "package_id": "path+file:///w/crates/app#agent-interface-app@0.3.0",
            "target": {"name": "agent-interface"},
            "filenames": ["/w/target/debug/agent-interface"],
        }
        names = sweep.unit_names(binary)
        entries = [
            "agent_interface-3333333333333333",
            "agent-interface-app-4444444444444444",
            "other-5555555555555555",
        ]
        self.assertEqual(list(sweep.unused(entries, set(), names)), ["other-5555555555555555"])


if __name__ == "__main__":
    unittest.main()
