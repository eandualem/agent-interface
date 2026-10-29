"""Exercise installer transactions with inert payloads in private homes."""

import hashlib
import io
import os
from pathlib import Path
import subprocess
import tarfile
import tempfile
import unittest

INSTALL = Path(__file__).resolve().parents[2] / "install.sh"
APP_ID = "io.github.eandualem.agent-interface"


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
        self.installer = INSTALL
        self.destination = self.home / ".local/lib/agent-interface"
        self.launcher = self.home / f".local/share/applications/{APP_ID}.desktop"
        self.link = self.home / ".local/bin/agent-interface"

    def command(self, name, body):
        path = self.tools / name
        path.write_text("#!/bin/sh\n" + body + "\n")
        path.chmod(0o755)

    def package(self, version="0.0.0", target="x86_64-unknown-linux-gnu", extra=None, legacy_mac=False, legacy=False):
        archive = self.root / "app.tar.gz"
        mac = target.endswith("apple-darwin")
        name = "tmux-workspaces" if legacy else "agent-interface"
        bundle = "tmux-workspaces.app" if legacy_mac else "Agent Interface.app"
        root = bundle + "/Contents" if mac else name
        resources = root + "/Resources" if mac else root
        executable = root + ("/MacOS" if mac else "/bin") + "/" + name
        files = {resources + "/INSTALLER": "io.github.eandualem." + name + "\n",
                 resources + "/TARGET": target + "\n",
                 executable: f"#!/bin/sh\necho '{name} {version}'\n"}
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
        result = subprocess.run(["sh", str(self.installer), *args], env=self.env, capture_output=True, text=True)
        self.assertEqual(result.returncode == 0, success, result.stdout + result.stderr)
        return result

    def wsl(self):
        # Redirect the one system path into the fixture. sudo is always inert;
        # these tests never write system launchers or install real packages.
        self.system_apps = self.root / "system-applications"
        self.system_apps.mkdir()
        self.installer = self.root / "install.sh"
        self.installer.write_text(INSTALL.read_text().replace(
            "/usr/share/applications", str(self.system_apps)))
        self.env.update(WSL_DISTRO_NAME="Ubuntu", DISPLAY=":0",
                        TEST_SYSTEM_ACTIONS=str(self.root / "system-actions"))
        self.command("dpkg-query", "exit 1")
        self.command("apt-get", "exit 99")
        self.command("sudo", '''
printf '%s\\n' "$*" >> "$TEST_SYSTEM_ACTIONS"
case "$1" in
    -v) exit 0;;
    apt-get) exit 0;;
    install)
        if [ "${TEST_FAIL_SHORTCUT:-}" = yes ]; then exit 1; fi
        shift; exec /usr/bin/install "$@";;
    rm) shift; exec /bin/rm "$@";;
    *) exit 99;;
esac
''')
        return self.system_apps / f"{APP_ID}.desktop"

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

    def mock_download(self, archive, prefix="tmux-workspaces", missing_status="404"):
        self.env.update(TEST_ARCHIVE=str(archive), TEST_PREFIX=prefix,
                        TEST_DOWNLOADS=str(self.root / "downloads"), TEST_MISSING_STATUS=missing_status)
        self.command("curl", '''
url= output= write_status=false
while [ "$#" -gt 0 ]; do
    case "$1" in
        -o) output=$2; shift 2;;
        --write-out) write_status=true; shift 2;;
        https://*) url=$1; shift;;
        *) shift;;
    esac
done
name=${url##*/}
printf '%s\\n' "$name" >> "$TEST_DOWNLOADS"
case "$name" in
    "$TEST_PREFIX"-*.tar.gz) cp "$TEST_ARCHIVE" "$output";;
    "$TEST_PREFIX"-*.tar.gz.sha256) cp "$TEST_ARCHIVE.sha256" "$output";;
    *) if [ "$write_status" = true ]; then printf '%s' "$TEST_MISSING_STATUS"; fi; exit 22;;
esac
if [ "$write_status" = true ]; then printf 200; fi
''')

    def test_download_legacy_linux_then_update_to_renamed_release(self):
        archive = self.package("0.1.0", legacy=True)
        self.mock_download(archive)
        self.install(None)
        self.assertEqual(subprocess.check_output([self.link, "--version"], text=True).strip(), "tmux-workspaces 0.1.0")
        self.assertIn(f"Icon={self.destination}/agent-interface.png\n", self.launcher.read_text())
        self.assertTrue((self.destination / "agent-interface.png").is_file())
        self.assertIn("StartupWMClass=io.github.eandualem.tmux-workspaces\n", self.launcher.read_text())
        self.assertEqual((self.root / "downloads").read_text().splitlines(), [
            "agent-interface-x86_64-unknown-linux-gnu.tar.gz",
            "tmux-workspaces-x86_64-unknown-linux-gnu.tar.gz",
            "tmux-workspaces-x86_64-unknown-linux-gnu.tar.gz.sha256"])
        archive = self.package("0.1.1")
        self.mock_download(archive, prefix="agent-interface")
        self.install(None)
        self.assertEqual(subprocess.check_output([self.link, "--version"], text=True).strip(), "agent-interface 0.1.1")
        self.assertIn("StartupWMClass=io.github.eandualem.agent-interface\n", self.launcher.read_text())

    def test_download_legacy_mac_then_update_to_renamed_release(self):
        self.command("uname", 'case "$1" in -s) echo Darwin;; -m) echo arm64;; esac')
        self.command("codesign", "exit 0")
        archive = self.package("0.1.0", target="aarch64-apple-darwin", legacy=True)
        self.mock_download(archive)
        self.install(None)
        destination = self.home / "Applications/Agent Interface.app/Contents/MacOS"
        self.assertEqual(subprocess.check_output([destination / "tmux-workspaces", "--version"], text=True).strip(), "tmux-workspaces 0.1.0")
        self.install(self.package("0.1.1", target="aarch64-apple-darwin"))
        self.assertEqual(subprocess.check_output([destination / "agent-interface", "--version"], text=True).strip(), "agent-interface 0.1.1")

    def test_download_failure_does_not_fall_back_or_change_installation(self):
        self.install(self.package("0.1.1"))
        self.mock_download(self.package("0.1.0", legacy=True), missing_status="500")
        self.install(None, success=False)
        self.assertEqual(subprocess.check_output([self.link, "--version"], text=True).strip(), "agent-interface 0.1.1")
        self.assertEqual((self.root / "downloads").read_text().splitlines(), ["agent-interface-x86_64-unknown-linux-gnu.tar.gz"])

    def test_legacy_download_checksum_failure_preserves_installation(self):
        self.install(self.package("0.1.1"))
        archive = self.package("0.1.0", legacy=True)
        with archive.open("ab") as stream:
            stream.write(b"corrupt")
        self.mock_download(archive)
        self.install(None, success=False)
        self.assertEqual(subprocess.check_output([self.link, "--version"], text=True).strip(), "agent-interface 0.1.1")

    def test_transitional_link_is_restored_after_update_failure(self):
        self.install(self.package("0.1.0", legacy=True))
        before = self.launcher.read_bytes()
        self.command("mv", 'for last do :; done\ncase "$last" in *.desktop)\n'
                     'case "$*" in *launcher.*) echo partial > "$last"; exit 1;; esac;; esac\n'
                     'exec /bin/mv "$@"')
        self.install(self.package("0.1.1"), success=False)
        self.assertEqual(subprocess.check_output([self.link, "--version"], text=True).strip(), "tmux-workspaces 0.1.0")
        self.assertEqual(self.launcher.read_bytes(), before)

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

    def legacy_linux_install(self):
        old = self.home / ".local/lib/tmux-workspaces"
        binary = old / "bin/tmux-workspaces"
        binary.parent.mkdir(parents=True)
        binary.write_text("#!/bin/sh\necho 'tmux-workspaces 0.1.0'\n")
        binary.chmod(0o755)
        (old / "INSTALLER").write_text("io.github.eandualem.tmux-workspaces\n")
        link = self.home / ".local/bin/tmux-workspaces"
        link.parent.mkdir(parents=True)
        link.symlink_to(binary)
        launcher = self.launcher.with_name("io.github.eandualem.tmux-workspaces.desktop")
        launcher.parent.mkdir(parents=True)
        launcher.write_text("[Desktop Entry]\nX-TmuxWorkspaces-Installer=1\n")
        library = self.home / ".local/share/tmux-workspaces/layouts.db"
        library.parent.mkdir(parents=True)
        library.write_bytes(b"existing workspace")
        return old, link, launcher, library

    def test_linux_rename_retires_owned_launchers_and_preserves_library(self):
        old, link, launcher, library = self.legacy_linux_install()
        self.install(self.package())
        for retired in (old, link, launcher):
            self.assertFalse(retired.exists())
            self.assertFalse(retired.is_symlink())
        self.assertEqual(subprocess.check_output([self.link, "--version"], text=True).strip(), "agent-interface 0.0.0")
        self.assertEqual(library.read_bytes(), b"existing workspace")

    def test_linux_rename_failure_restores_old_install_and_launchers(self):
        old, link, launcher, library = self.legacy_linux_install()
        self.command("rm", 'for last do :; done\ncase "$last" in *tmux-workspaces.desktop) exit 1;; esac\nexec /bin/rm "$@"')
        self.install(self.package(), success=False)
        self.assertTrue(old.exists())
        self.assertTrue(launcher.exists())
        self.assertEqual(subprocess.check_output([link, "--version"], text=True).strip(), "tmux-workspaces 0.1.0")
        self.assertEqual(library.read_bytes(), b"existing workspace")
        self.assertFalse(self.destination.exists())
        self.assertFalse(self.link.is_symlink())
        self.assertFalse(self.launcher.exists())

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

    def test_mac_rename_update_and_rollback(self):
        self.command("uname", 'case "$1" in -s) echo Darwin;; -m) echo arm64;; esac')
        self.command("codesign", "exit 0")
        destination = self.home / "Applications/Agent Interface.app"
        legacy = self.home / "Applications/tmux-workspaces.app"
        self.install(self.package(target="aarch64-apple-darwin", legacy_mac=True))
        destination.rename(legacy)
        # Failed replacement restores the old bundle under its original name.
        self.command("mv", 'case "$1" in */"Agent Interface.app") exit 1;; esac\nexec /bin/mv "$@"')
        self.install(self.package("0.0.1", target="aarch64-apple-darwin"), success=False)
        self.assertTrue(legacy.exists())
        self.assertFalse(destination.exists())
        (self.tools / "mv").unlink()
        self.install(self.package("0.0.1", target="aarch64-apple-darwin"))
        self.assertFalse(legacy.exists())
        binary = destination / "Contents/MacOS/agent-interface"
        self.assertEqual(subprocess.check_output([binary, "--version"], text=True).strip(), "agent-interface 0.0.1")
        self.install(self.package("0.0.2", target="aarch64-apple-darwin"))
        self.assertEqual(subprocess.check_output([binary, "--version"], text=True).strip(), "agent-interface 0.0.2")

    def test_mac_preserves_unmanaged_legacy_app(self):
        self.command("uname", 'case "$1" in -s) echo Darwin;; -m) echo arm64;; esac')
        legacy = self.home / "Applications/tmux-workspaces.app"
        legacy.mkdir(parents=True)
        (legacy / "mine").write_text("preserve")
        self.install(self.package(target="aarch64-apple-darwin"), success=False)
        self.assertEqual((legacy / "mine").read_text(), "preserve")
        self.assertFalse((self.home / "Applications/Agent Interface.app").exists())

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


    def backbone_tools(self):
        self.env.update(FIXTURE_TOOLS=str(self.root / "uv tools"),
                        FIXTURE_LOG=str(self.root / "provider.log"),
                        FIXTURE_PYTHON=str(self.tools / "provider-python"))
        self.command("provider-python", '''
printf '%s\\n' "$*" >> "$FIXTURE_LOG"
case "$*" in *--resume-pending*) exit "${FIXTURE_RESUME:-0}";; esac
exit "${FIXTURE_SETUP_FAIL:-0}"
''')
        self.command("uv", '''
printf '%s\\n' "$*" >> "$FIXTURE_LOG"
case "$1 $2" in
    "tool dir") printf '%s\\n' "$FIXTURE_TOOLS";;
    "tool install")
        [ "${FIXTURE_INSTALL_FAIL:-0}" = 0 ] || exit 1
        mkdir -p "$FIXTURE_TOOLS/agent-backbone/bin"
        cp "$FIXTURE_PYTHON" "$FIXTURE_TOOLS/agent-backbone/bin/python"
        chmod 755 "$FIXTURE_TOOLS/agent-backbone/bin/python";;
    "tool update-shell") :;;
    *) exit 1;;
esac
''')

    def test_latest_provider_install_and_pending_resume_order(self):
        self.backbone_tools()
        archive = self.package("0.2.0", extra={"agent-interface/setup_backbone.py": "fixture helper"})
        self.install(archive)
        log = Path(self.env["FIXTURE_LOG"])
        lines = log.read_text().splitlines()
        self.assertIn("tool install --upgrade --prerelease disallow --python >=3.11,<3.14 agent-backbone>=0.4.0", lines)
        self.assertTrue(lines[-1].startswith("-I -B "))
        self.env["FIXTURE_RESUME"] = "3"
        log.write_text("")
        self.install(archive)
        lines = log.read_text().splitlines()
        self.assertTrue(any("--resume-pending" in line for line in lines))
        self.assertFalse(any(line.startswith("tool install") for line in lines))

    def test_provider_failure_keeps_previous_app(self):
        self.install(self.package("0.1.1"))
        self.backbone_tools()
        archive = self.package("0.2.0", extra={"agent-interface/setup_backbone.py": "fixture helper"})
        self.env["FIXTURE_INSTALL_FAIL"] = "1"
        self.install(archive, success=False)
        self.assertIn("0.1.1", subprocess.check_output([self.link, "--version"], text=True))
        self.env["FIXTURE_INSTALL_FAIL"] = "0"
        self.env["FIXTURE_SETUP_FAIL"] = "1"
        self.install(archive, success=False)
        self.assertIn("0.1.1", subprocess.check_output([self.link, "--version"], text=True))
        self.env["FIXTURE_RESUME"] = "1"
        Path(self.env["FIXTURE_LOG"]).write_text("")
        self.install(archive, success=False)
        self.assertNotIn("tool install", Path(self.env["FIXTURE_LOG"]).read_text())


if __name__ == "__main__":
    unittest.main()
