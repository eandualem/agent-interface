#!/usr/bin/env python3
"""Build tmux with pinned static dependencies on a matching macOS/Linux host.

Requires a system C toolchain, make and Python 3.12 or later. Never
starts a server. Dependencies are built in a private prefix, not installed on
the host. The output is a build input; it is not installed or published.
"""

import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import shutil
import subprocess
import tarfile
import tempfile
import urllib.request

ROOT = Path(__file__).resolve().parent.parent

TARGETS = {f"{arch}-{system}" for arch in ("aarch64", "x86_64")
           for system in ("apple-darwin", "unknown-linux-gnu")}


def native_target(expected=None):
    system = {"Darwin": "apple-darwin", "Linux": "unknown-linux-gnu"}.get(platform.system())
    arch = {"arm64": "aarch64", "aarch64": "aarch64", "x86_64": "x86_64"}.get(platform.machine())
    if system is None or arch is None:
        raise ValueError("Unsupported native build host")
    if system == "unknown-linux-gnu" and platform.libc_ver()[0] != "glibc":
        raise ValueError("Linux builds require a glibc host")
    target = f"{arch}-{system}"
    if expected is not None and expected != target:
        raise ValueError("Expected target does not match the native build host")
    return target


def binary_report(binary, target):
    """Inspect without executing the binary; runtime acceptance is separate."""
    if target not in TARGETS:
        raise ValueError("Unsupported binary target")

    def output(*args):
        return subprocess.check_output(args, text=True, env={"PATH": "/usr/bin:/bin", "LC_ALL": "C"})

    if target.endswith("apple-darwin"):
        expected = "arm64" if target.startswith("aarch64-") else "x86_64"
        if output("/usr/bin/lipo", "-archs", str(binary)).split() != [expected]:
            raise ValueError("tmux Mach-O architecture does not match target")
        deps = output("/usr/bin/otool", "-L", str(binary))
        libraries = [line.strip().split(" (compatibility", 1)[0]
                     for line in deps.splitlines() if " (compatibility" in line]
        if not libraries or any(not name.startswith("/usr/lib/") for name in libraries):
            raise ValueError("tmux links a non-system library: " + deps)
        commands = output("/usr/bin/otool", "-l", str(binary))
        versions = re.findall(r"\bminos\s+(\d+(?:\.\d+)+)", commands)
        versions += re.findall(r"cmd LC_VERSION_MIN_MACOSX\s+cmdsize \d+\s+version (\d+(?:\.\d+)+)", commands)
        if not versions or any(tuple(map(int, value.split("."))) > (11, 0, 0)
                               for value in versions):
            raise ValueError("tmux minimum macOS exceeds 11.0 or is missing")
        if "_pipe2" in output("/usr/bin/nm", "-u", str(binary)).split():
            raise ValueError("tmux unexpectedly requires macOS 27 pipe2")
        return {"format": "Mach-O", "architecture": expected,
                "minimum_macos": versions, "libraries": libraries}

    header = output("/usr/bin/readelf", "-hW", str(binary))
    machine = "AArch64" if target.startswith("aarch64-") else "Advanced Micro Devices X86-64"
    if (not re.search(r"Class:\s+ELF64\b", header)
            or not re.search(r"Data:\s+2's complement, little endian", header)
            or not re.search(r"Machine:\s+" + re.escape(machine) + r"\s*$", header, re.M)
            or not re.search(r"Type:\s+(EXEC|DYN)\b", header)):
        raise ValueError("tmux ELF architecture or format does not match target")
    dynamic = output("/usr/bin/readelf", "-dW", str(binary))
    libraries = re.findall(r"\(NEEDED\).*?\[([^\]]+)\]", dynamic)
    allowed = {"libc.so.6", "libm.so.6", "libdl.so.2", "libpthread.so.0",
               "librt.so.1", "libutil.so.1", "libresolv.so.2"}
    if "libc.so.6" not in libraries or set(libraries) - allowed:
        raise ValueError("tmux ELF links unexpected or non-glibc libraries")
    if re.search(r"\((RPATH|RUNPATH)\)", dynamic):
        raise ValueError("tmux ELF must not embed a library search path")
    program = output("/usr/bin/readelf", "-lW", str(binary))
    interpreters = re.findall(r"\[Requesting program interpreter: ([^\]]+)\]", program)
    expected = "/lib/ld-linux-aarch64.so.1" if target.startswith("aarch64-") else "/lib64/ld-linux-x86-64.so.2"
    if interpreters != [expected]:
        raise ValueError("tmux ELF interpreter does not match the glibc target")
    versions = output("/usr/bin/readelf", "--version-info", "-W", str(binary))
    if "GLIBC_PRIVATE" in versions:
        raise ValueError("tmux ELF uses a private glibc ABI")
    required = sorted(set(re.findall(r"\bGLIBC_(\d+(?:\.\d+)+)\b", versions)),
                      key=lambda value: tuple(map(int, value.split("."))))
    if not required:
        raise ValueError("tmux ELF has no verifiable glibc version requirements")
    return {"format": "ELF", "architecture": machine, "interpreter": expected,
            "libraries": libraries, "glibc_versions": required, "minimum_glibc": required[-1]}


def fetch(item, cache):
    archive = cache / item["name"]
    if not archive.exists():
        with urllib.request.urlopen(item["url"], timeout=120) as response:
            with tempfile.NamedTemporaryFile(dir=cache, delete=False) as stream:
                temporary = Path(stream.name)
                try:
                    shutil.copyfileobj(response, stream)
                    stream.flush()
                    if hashlib.sha256(temporary.read_bytes()).hexdigest() != item["sha256"]:
                        raise ValueError("Source checksum mismatch: " + item["name"])
                    temporary.replace(archive)
                finally:
                    temporary.unlink(missing_ok=True)
    if hashlib.sha256(archive.read_bytes()).hexdigest() != item["sha256"]:
        raise ValueError("Source checksum mismatch: " + item["name"])
    return archive


def build(destination, cache, expected_target=None):
    target = native_target(expected_target)
    mac = target.endswith("apple-darwin")
    if destination.exists() or destination.is_symlink():
        raise ValueError("Output already exists")
    sources = json.loads((ROOT / "packaging/tmux-sources.json").read_text())
    cache.mkdir(parents=True, exist_ok=True)
    archives = [fetch(item, cache) for item in sources]
    # Autoconf and Make do not consistently quote their build/prefix paths.
    with tempfile.TemporaryDirectory(prefix="agent-interface-tmux-", dir="/tmp") as temporary:
        work = Path(temporary)
        prefix = work / "prefix"
        prefix.mkdir()
        for archive in archives:
            with tarfile.open(archive) as tar:
                tar.extractall(work, filter="data")
        env = {key: value for key, value in os.environ.items() if key in {
            "HOME", "TMPDIR",
        }}
        env.update(
            PATH="/usr/bin:/bin:/usr/sbin:/sbin", LC_ALL="C",
            CC="/usr/bin/clang" if mac else "/usr/bin/cc",
            CFLAGS="-O2 -ffile-prefix-map=" + str(work) + "=/build/tmux",
            LDFLAGS="",
            PKG_CONFIG_LIBDIR=str(prefix / "lib/pkgconfig"),
            PKG_CONFIG_PATH=str(prefix / "lib/pkgconfig"),
        )
        if mac:
            env.update({key: os.environ[key] for key in ("DEVELOPER_DIR", "SDKROOT") if key in os.environ})
            env.update(MACOSX_DEPLOYMENT_TARGET="11.0",
                       LDFLAGS="-mmacosx-version-min=11.0",
                       CFLAGS=env["CFLAGS"] + " -mmacosx-version-min=11.0 -Werror=unguarded-availability-new",
            # AC_CHECK_FUNCS links without including the availability-marked
            # declaration. A new SDK can therefore falsely enable pipe2.
                       ac_cv_func_pipe2="no")

        def run(folder, *args, extra=None):
            subprocess.run(args, cwd=work / folder, env=env | (extra or {}), check=True)

        def make(folder):
            run(folder, "make", "-j4")
            run(folder, "make", "install")

        run("libevent-2.1.12-stable", "./configure", "--prefix=" + str(prefix),
            "--disable-shared", "--disable-openssl", "--disable-libevent-regress",
            "--disable-samples")
        make("libevent-2.1.12-stable")
        run("ncurses-6.5", "./configure", "--prefix=" + str(prefix), "--enable-widec",
            "--without-shared", "--without-ada", "--without-cxx", "--without-cxx-binding",
            "--without-tests", "--without-manpages", "--without-debug", "--disable-db-install",
            "--with-termlib", "--with-default-terminfo-dir=/usr/share/terminfo",
            "--with-terminfo-dirs=/etc/terminfo:/lib/terminfo:/usr/share/terminfo")
        make("ncurses-6.5")
        run("utf8proc-2.11.2", "make", "-j4", "libutf8proc.a")
        shutil.copyfile(work / "utf8proc-2.11.2/libutf8proc.a", prefix / "lib/libutf8proc.a")
        shutil.copyfile(work / "utf8proc-2.11.2/utf8proc.h", prefix / "include/utf8proc.h")
        libraries = {}
        for label, archive, include in [
            ("LIBEVENT", "libevent.a", "include"),
            ("LIBNCURSES", "libncursesw.a", "include/ncursesw"),
            ("LIBTINFO", "libtinfow.a", "include/ncursesw"),
            ("LIBUTF8PROC", "libutf8proc.a", "include"),
        ]:
            libraries[label + "_CFLAGS"] = "-I" + str(prefix / include)
            libraries[label + "_LIBS"] = str(prefix / "lib" / archive)
        run("tmux-3.8", "./configure", "--prefix=" + str(prefix),
            "--sysconfdir=/etc", "--enable-utf8proc", "--disable-jemalloc", extra=libraries)
        run("tmux-3.8", "make", "-j4", extra=libraries)
        binary = work / "tmux-3.8/tmux"
        report = binary_report(binary, target)
        destination.mkdir(parents=True, exist_ok=False)
        shutil.copy2(binary, destination / "tmux")
        notices = []
        for folder, names in [
            ("tmux-3.8", ["COPYING"]),
            ("libevent-2.1.12-stable", ["LICENSE"]),
            ("ncurses-6.5", ["COPYING"]),
            ("utf8proc-2.11.2", ["LICENSE.md"]),
        ]:
            for name in names:
                notices.append(folder + " / " + name + "\n\n" + (work / folder / name).read_text())
        (destination / "LICENSES.txt").write_text("\n\n".join(notices))
        (destination / "BUILD.json").write_text(json.dumps({
            "schema_version": 1, "version": "3.8", "machine": platform.machine(),
            "target": target, "minimum_macos": "11.0" if mac else None, "sources": sources,
            "compiler": subprocess.check_output([env["CC"], "--version"], env=env, text=True).splitlines()[0],
            "binary": report,
            "sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
        }, indent=2) + "\n")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True, help="New output directory")
    parser.add_argument("--cache", type=Path, default=ROOT / ".native/tmux-downloads")
    parser.add_argument("--target", choices=sorted(TARGETS), help="Require this native host target; cross-compilation is unsupported")
    args = parser.parse_args()
    build(args.output.absolute(), args.cache.resolve(), args.target)
