"""Source pinning and native artifact rejection, without executing binaries."""

import hashlib
import importlib.util
import io
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location(
    "prepare_tmux", Path(__file__).resolve().parents[1] / "prepare-tmux.py")
builder = importlib.util.module_from_spec(spec)
spec.loader.exec_module(builder)


class DownloadTests(unittest.TestCase):
    def test_cache_miss_publishes_only_verified_source(self):
        data = b"source archive fixture"
        item = {"name": "source.tar.gz", "url": "https://example.invalid/source",
                "sha256": hashlib.sha256(data).hexdigest()}
        with tempfile.TemporaryDirectory() as temporary:
            cache = Path(temporary)
            with patch.object(builder.urllib.request, "urlopen", return_value=io.BytesIO(data)) as fetch:
                archive = builder.fetch(item, cache)
                fetch.assert_called_once_with(item["url"], timeout=120)
            self.assertEqual(archive.read_bytes(), data)
            with patch.object(builder.urllib.request, "urlopen") as fetch:
                self.assertEqual(builder.fetch(item, cache), archive)
                fetch.assert_not_called()
            self.assertEqual(list(cache.iterdir()), [archive])

    def test_bad_download_leaves_no_cached_or_temporary_file(self):
        item = {"name": "source.tar.gz", "url": "https://example.invalid/source",
                "sha256": "0" * 64}
        with tempfile.TemporaryDirectory() as temporary:
            cache = Path(temporary)
            with patch.object(builder.urllib.request, "urlopen", return_value=io.BytesIO(b"bad")):
                with self.assertRaisesRegex(ValueError, "checksum mismatch"):
                    builder.fetch(item, cache)
            self.assertEqual(list(cache.iterdir()), [])


class TargetTests(unittest.TestCase):
    def test_native_targets_and_mismatched_expected_target(self):
        for system, machine, target in [
            ("Darwin", "arm64", "aarch64-apple-darwin"),
            ("Darwin", "x86_64", "x86_64-apple-darwin"),
            ("Linux", "aarch64", "aarch64-unknown-linux-gnu"),
            ("Linux", "x86_64", "x86_64-unknown-linux-gnu"),
        ]:
            with self.subTest(target=target), patch.object(builder.platform, "system", return_value=system), \
                    patch.object(builder.platform, "machine", return_value=machine), \
                    patch.object(builder.platform, "libc_ver", return_value=("glibc", "2.39")):
                self.assertEqual(builder.native_target(target), target)
                with self.assertRaisesRegex(ValueError, "does not match"):
                    builder.native_target("different-target")

    def test_musl_and_unknown_hosts_refused_before_fetch(self):
        with patch.object(builder.platform, "system", return_value="Linux"), \
                patch.object(builder.platform, "machine", return_value="x86_64"), \
                patch.object(builder.platform, "libc_ver", return_value=("musl", "1.2")), \
                patch.object(builder, "fetch") as fetch:
            with self.assertRaisesRegex(ValueError, "glibc"):
                builder.build(Path("unused"), Path("unused-cache"))
            fetch.assert_not_called()
        with patch.object(builder.platform, "system", return_value="Windows"):
            with self.assertRaisesRegex(ValueError, "Unsupported"):
                builder.native_target()


class BinaryTests(unittest.TestCase):
    def elf_outputs(self, arm=False):
        return {
            "-hW": "Class: ELF64\nData: 2's complement, little endian\nType: DYN (Position-Independent Executable file)\nMachine: " + ("AArch64" if arm else "Advanced Micro Devices X86-64") + "\n",
            "-dW": "0x1 (NEEDED) Shared library: [libc.so.6]\n0x1 (NEEDED) Shared library: [libm.so.6]\n",
            "-lW": "[Requesting program interpreter: " + ("/lib/ld-linux-aarch64.so.1" if arm else "/lib64/ld-linux-x86-64.so.2") + "]\n",
            "--version-info": "Name: GLIBC_2.9\nName: GLIBC_2.34\nName: GLIBC_2.17\n",
        }

    def inspect_elf(self, outputs, arm=False):
        with patch.object(builder.subprocess, "check_output", side_effect=lambda args, **kw: outputs[args[1]]):
            return builder.binary_report(Path("fixture"), ("aarch64" if arm else "x86_64") + "-unknown-linux-gnu")

    def test_both_elf_architectures_record_actual_glibc_requirements(self):
        for arm in (False, True):
            result = self.inspect_elf(self.elf_outputs(arm), arm)
            self.assertEqual(result["minimum_glibc"], "2.34")
            self.assertEqual(result["glibc_versions"], ["2.9", "2.17", "2.34"])

    def test_elf_wrong_architecture_loader_and_dynamic_dependency_rejected(self):
        invalid = [
            ("-hW", self.elf_outputs(True)["-hW"]),
            ("-hW", self.elf_outputs()["-hW"].replace("ELF64", "ELF32")),
            ("-dW", "0x1 (NEEDED) Shared library: [libevent.so.2]\n"),
            ("-dW", self.elf_outputs()["-dW"] + "0x1 (RUNPATH) Library runpath: [/tmp/build]\n"),
            ("-dW", self.elf_outputs()["-dW"] + "0x1 (RPATH) Library rpath: [$ORIGIN]\n"),
            ("-lW", "[Requesting program interpreter: /lib/ld-musl-x86_64.so.1]\n"),
            ("--version-info", "Name: GLIBC_PRIVATE\n"),
            ("--version-info", "No version information found\n"),
        ]
        for field, value in invalid:
            with self.subTest(field=field, value=value):
                outputs = self.elf_outputs(); outputs[field] = value
                with self.assertRaises(ValueError):
                    self.inspect_elf(outputs)

    def mach_outputs(self):
        return {"-archs": "arm64\n", "-L": "fixture:\n\t/usr/lib/libSystem.B.dylib (compatibility version 1.0.0, current version 1.0.0)\n",
                "-l": "cmd LC_BUILD_VERSION\nplatform 1\nminos 11.0\nsdk 27.0\n", "-u": "_close\n"}

    def inspect_mach(self, outputs, intel=False):
        with patch.object(builder.subprocess, "check_output", side_effect=lambda args, **kw: outputs[args[1]]):
            return builder.binary_report(Path("fixture"), ("x86_64" if intel else "aarch64") + "-apple-darwin")

    def test_macos_native_architectures_and_legacy_minimum_version(self):
        self.assertEqual(self.inspect_mach(self.mach_outputs())["minimum_macos"], ["11.0"])
        outputs = self.mach_outputs(); outputs["-archs"] = "x86_64\n"
        outputs["-l"] = "cmd LC_VERSION_MIN_MACOSX\ncmdsize 16\nversion 11.0\nsdk 14.0\n"
        self.assertEqual(self.inspect_mach(outputs, True)["architecture"], "x86_64")

    def test_macos_newer_abi_non_system_library_and_wrong_architecture_rejected(self):
        for field, value in [("-archs", "x86_64\n"), ("-archs", "x86_64 arm64\n"),
                             ("-L", "fixture:\n\t/opt/local/lib/libevent.dylib (compatibility version 1.0.0)\n"),
                             ("-l", "minos 12.0\n"), ("-l", "sdk 27.0\n"), ("-u", "_pipe2\n")]:
            with self.subTest(field=field, value=value):
                outputs = self.mach_outputs(); outputs[field] = value
                with self.assertRaises(ValueError):
                    self.inspect_mach(outputs)
