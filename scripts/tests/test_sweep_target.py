"""Check which artifacts the sweep keeps."""

import importlib.util
import tempfile
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
            list(sweep.unused(entries, hashes)),
            ["libfoo-1111111111111111.rlib", "bar-2222222222222222"],
        )

    def test_finds_the_hash_of_a_binary_copied_out_without_it(self):
        with tempfile.TemporaryDirectory() as root:
            debug = Path(root)
            (debug / "deps").mkdir()
            (debug / "agent-interface").write_bytes(b"current")
            (debug / "deps" / "agent_interface-aaaaaaaaaaaaaaaa").write_bytes(b"current")
            (debug / "deps" / "agent_interface-bbbbbbbbbbbbbbbb").write_bytes(b"earlier")
            binary = {
                "reason": "compiler-artifact",
                "filenames": [str(debug / "agent-interface")],
            }
            self.assertEqual(sweep.copied_hashes(binary), {"aaaaaaaaaaaaaaaa"})


if __name__ == "__main__":
    unittest.main()
