#!/usr/bin/env python3
"""Prepare the pinned, patched terminal source without editing a caller's checkout."""

import hashlib
import json
import os
from pathlib import Path
import subprocess
import tempfile


ROOT = Path(__file__).resolve().parents[1]
MANIFEST = json.loads((ROOT / "native/ghostty.json").read_text())
PATCH = ROOT / "native/ghostty.patch"


def verify(source):
    try:
        str(source).encode("utf-8")
    except UnicodeEncodeError:
        raise SystemExit("The native binding requires a UTF-8 source path") from None
    for name, expected in MANIFEST["files"].items():
        path = source / name
        if (
            not path.is_file()
            or hashlib.sha256(path.read_bytes()).hexdigest() != expected
        ):
            raise SystemExit(f"Native source does not match the required patch: {path}")


def main():
    digest = hashlib.sha256(PATCH.read_bytes()).hexdigest()
    if digest != MANIFEST["patch_sha256"]:
        raise SystemExit("native/ghostty.patch does not match native/ghostty.json")
    override = os.environ.get("GHOSTTY_SOURCE_DIR")
    if override is not None:
        source = Path(override).resolve()
        verify(source)
        print(source)
        return

    cache = ROOT / ".native"
    destination = cache / f"ghostty-{MANIFEST['commit']}-{digest}"
    if destination.exists():
        verify(destination)
        print(destination)
        return
    cache.mkdir(exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="prepare-", dir=cache) as temporary:
        source = Path(temporary) / "source"
        source.mkdir()
        env = dict(os.environ, GIT_TERMINAL_PROMPT="0")

        def git(*args):
            subprocess.run(["git", *args], cwd=source, env=env, check=True)

        git("init", "--quiet")
        git(
            "fetch", "--quiet", "--depth=1",
            "https://github.com/ghostty-org/ghostty.git", MANIFEST["commit"],
        )
        git("checkout", "--quiet", "--detach", "FETCH_HEAD")
        git("apply", "--check", str(PATCH))
        git("apply", str(PATCH))
        verify(source)
        try:
            source.rename(destination)
        except OSError:
            # Another preparation may have finished first. Never replace it.
            if not destination.is_dir():
                raise
            verify(destination)
    print(destination)


if __name__ == "__main__":
    main()
