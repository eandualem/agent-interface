#!/usr/bin/env python3
"""Build the pinned, statically linked tmux payload on macOS.

Requires the Apple command-line tools, make and Python 3.12 or later. Never
starts a server. Dependencies are built in a private prefix, not installed on
the host. The output is a build input; it is not installed or published.
"""

import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import tarfile
import tempfile
import urllib.request

ROOT = Path(__file__).resolve().parent.parent


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


def build(destination, cache):
    if platform.system() != "Darwin":
        raise SystemExit("This builder currently supports macOS hosts only.")
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
            "HOME", "TMPDIR", "LANG", "LC_ALL", "DEVELOPER_DIR", "SDKROOT",
        }}
        env.update(
            PATH="/usr/bin:/bin:/usr/sbin:/sbin", CC="/usr/bin/clang",
            MACOSX_DEPLOYMENT_TARGET="11.0",
            CFLAGS=("-O2 -mmacosx-version-min=11.0 -Werror=unguarded-availability-new "
                    "-ffile-prefix-map=" + str(work) + "=/build/tmux"),
            LDFLAGS="-mmacosx-version-min=11.0",
            PKG_CONFIG_LIBDIR=str(prefix / "lib/pkgconfig"),
            PKG_CONFIG_PATH=str(prefix / "lib/pkgconfig"),
            # AC_CHECK_FUNCS links without including the availability-marked
            # declaration. A new SDK can therefore falsely enable pipe2.
            ac_cv_func_pipe2="no",
        )

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
        deps = subprocess.check_output(["otool", "-L", binary], text=True)
        if any(not line.strip().startswith("/usr/lib/") for line in deps.splitlines()[1:]):
            raise ValueError("tmux links a non-system library: " + deps)
        symbols = subprocess.check_output(["nm", "-u", binary], text=True).splitlines()
        if "_pipe2" in symbols:
            raise ValueError("tmux unexpectedly requires macOS 27 pipe2")
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
            "minimum_macos": "11.0", "sources": sources,
            "sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
        }, indent=2) + "\n")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True, help="New output directory")
    parser.add_argument("--cache", type=Path, default=ROOT / ".native/tmux-downloads")
    args = parser.parse_args()
    build(args.output.resolve(), args.cache.resolve())
