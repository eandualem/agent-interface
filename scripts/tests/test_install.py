"""Exercise installer transactions with inert Linux payloads in private homes."""

import hashlib
import io
import os
from pathlib import Path
import subprocess
import tarfile
import tempfile
import unittest

INSTALL = Path(__file__).resolve().parents[2] / "install.sh"
APP_ID = "io.github.eandualem.tmux-workspaces"


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
        self.destination = self.home / ".local/lib/tmux-workspaces"
        self.launcher = self.home / f".local/share/applications/{APP_ID}.desktop"
        self.link = self.home / ".local/bin/tmux-workspaces"

    def command(self, name, body):
        path = self.tools / name
        path.write_text("#!/bin/sh\n" + body + "\n")
        path.chmod(0o755)

    def package(self, version="0.0.0", target="x86_64-unknown-linux-gnu", extra=None):
        archive = self.root / "app.tar.gz"
        files = {"tmux-workspaces/INSTALLER": APP_ID + "\n",
                 "tmux-workspaces/TARGET": target + "\n",
                 "tmux-workspaces/bin/tmux-workspaces": f"#!/bin/sh\necho 'tmux-workspaces {version}'\n"}
        if extra:
            files.update(extra)
        with tarfile.open(archive, "w:gz") as tar:
            for name, text in files.items():
                info = tarfile.TarInfo(name)
                data = text.encode()
                info.size = len(data)
                info.mode = 0o755 if name.endswith("bin/tmux-workspaces") else 0o644
                tar.addfile(info, io.BytesIO(data))
        archive.with_suffix(".gz.sha256").write_text(hashlib.sha256(archive.read_bytes()).hexdigest() + "  app.tar.gz\n")
        return archive

    def install(self, archive, success=True):
        result = subprocess.run(["sh", str(INSTALL), "--archive", str(archive)], env=self.env, capture_output=True, text=True)
        self.assertEqual(result.returncode == 0, success, result.stdout + result.stderr)
        return result

    def test_install_update_and_preserve_data(self):
        library = self.home / ".local/share/tmux-workspaces/layouts.db"
        library.parent.mkdir(parents=True)
        library.write_bytes(b"preserve")
        self.install(self.package())
        self.assertEqual(subprocess.check_output([self.link, "--version"], text=True).strip(), "tmux-workspaces 0.0.0")
        self.assertIn(f"Icon={self.destination}/agent-interface.png\n", self.launcher.read_text())
        self.install(self.package("0.0.1"))
        self.assertEqual(subprocess.check_output([self.link, "--version"], text=True).strip(), "tmux-workspaces 0.0.1")
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
        self.install(self.package(extra={"tmux-workspaces/../../escape": "no"}), success=False)
        self.assertFalse((self.root / "escape").exists())
        self.assertFalse(self.destination.exists())

    def test_refuse_unmanaged_installation(self):
        self.destination.mkdir(parents=True)
        (self.destination / "mine").write_text("preserve")
        self.install(self.package(), success=False)
        self.assertEqual((self.destination / "mine").read_text(), "preserve")

    def test_rollback_after_launcher_failure(self):
        self.install(self.package())
        before = self.launcher.read_bytes()
        # Model a cross-filesystem move that damages the destination before failing.
        self.command("mv", 'for last do :; done\ncase "$last" in *.desktop)\n'
                     'case "$*" in *launcher.*) echo partial > "$last"; exit 1;; esac;; esac\n'
                     'exec /bin/mv "$@"')
        self.install(self.package("0.0.1"), success=False)
        self.assertEqual(subprocess.check_output([self.link, "--version"], text=True).strip(), "tmux-workspaces 0.0.0")
        self.assertEqual(self.launcher.read_bytes(), before)

    def test_preserve_launcher_backup_when_restore_fails(self):
        self.install(self.package())
        before = self.launcher.read_bytes()
        self.command("mv", 'for last do :; done\ncase "$last" in *.desktop) '
                     'echo partial > "$last"; exit 1;; esac\nexec /bin/mv "$@"')
        self.install(self.package("0.0.1"), success=False)
        self.assertEqual(subprocess.check_output([self.link, "--version"], text=True).strip(), "tmux-workspaces 0.0.0")
        backups = list(self.launcher.parent.glob(".tmux-workspaces-previous.*"))
        self.assertEqual(len(backups), 1)
        self.assertEqual(backups[0].read_bytes(), before)


if __name__ == "__main__":
    unittest.main()
