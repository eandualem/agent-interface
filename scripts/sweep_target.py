#!/usr/bin/env python3
"""Run the checks every commit needs, keeping only the build they used.

Cargo keeps every artifact it has ever built, so a checkout's target/ grows
without limit. This runs `cargo fmt --check`, `cargo clippy --all-targets
-- -D warnings`, builds the tests and `cargo build`, notes every artifact
cargo reports for them, whether it was rebuilt or still fresh, and removes
every other hashed artifact from target/debug. That keeps exactly the latest
build warm. It removes target/debug/incremental once that passes 2 GB. Then
it runs `cargo test`, whose result is the script's.

Arguments after `--` go to `cargo test`. Nothing is removed when a step
before the tests fails, since what failed to build was not reported, or
when target/ lies outside the checkout, where another checkout's build may
share it.

Usage: python3 scripts/sweep_target.py [-- TEST ARGS]
"""

import fcntl
import filecmp
import json
import re
import shutil
import subprocess
import sys
import time
from pathlib import Path

# A unit's metadata hash, as cargo writes it after the unit's name.
HASH = re.compile(r"-([0-9a-f]{16})(?=\.|$)")
INCREMENTAL_LIMIT = 2 * 1024**3
SWEPT = ("deps", ".fingerprint", "build", "examples")
LOCKS = (".cargo-lock", ".cargo-build-lock", ".cargo-artifact-lock")


def unit_hashes(message):
    """The unit hashes in the paths a cargo message names."""
    paths = list(message.get("filenames") or [])
    for key in ("executable", "out_dir"):
        if message.get(key):
            paths.append(message[key])
    return {
        match.group(1)
        for path in paths
        for part in Path(path).parts
        if (match := HASH.search(part))
    }


def copied_hashes(message):
    """The hash of an artifact cargo copied out under a plain name, such as
    the app's binary, found as the hashed file beside it or in deps/ with
    the same content. Cargo reports only the plain name."""
    found = set()
    if message.get("reason") != "compiler-artifact":
        return found
    for name in message.get("filenames") or []:
        path = Path(name)
        if HASH.search(path.name) or not path.is_file():
            continue
        stem = path.name.replace("-", "_")
        for folder in (path.parent / "deps", path.parent):
            if not folder.is_dir():
                continue
            for candidate in folder.iterdir():
                match = HASH.search(candidate.name)
                if (
                    match
                    and candidate.name == f"{stem}-{match.group(1)}"
                    and candidate.is_file()
                    and candidate.stat().st_size == path.stat().st_size
                    and filecmp.cmp(candidate, path, shallow=False)
                ):
                    found.add(match.group(1))
    return found


def unused(entries, hashes):
    """The entries no unit of the latest build owns."""
    for entry in entries:
        match = HASH.search(entry)
        if match and match.group(1) not in hashes:
            yield entry


def take_locks(debug):
    """Cargo's locks on the build directory, or None while a build holds one."""
    held = []
    for name in LOCKS:
        if not (debug / name).exists():
            continue
        handle = open(debug / name, "a")
        try:
            fcntl.flock(handle, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            handle.close()
            for other in held:
                other.close()
            return None
        held.append(handle)
    return held


def written_since(debug, moment):
    """Whether anything swept was written after `moment`, by another build."""
    return any(
        entry.lstat().st_mtime > moment
        for folder in SWEPT
        if (debug / folder).is_dir()
        for entry in (debug / folder).iterdir()
    )


def size(path):
    if path.is_symlink() or path.is_file():
        return path.lstat().st_size
    return sum(size(child) for child in path.iterdir()) if path.is_dir() else 0


def remove(path):
    if path.is_dir() and not path.is_symlink():
        shutil.rmtree(path)
    else:
        path.unlink()


def run(command, hashes):
    """Run one cargo step, showing its output, and note what it built."""
    process = subprocess.Popen(command, stdout=subprocess.PIPE, text=True)
    for line in process.stdout:
        try:
            message = json.loads(line)
        except ValueError:
            message = None
        if not isinstance(message, dict) or "reason" not in message:
            sys.stdout.write(line)
        elif message["reason"] == "compiler-message":
            sys.stdout.write(message["message"].get("rendered") or "")
        elif message["reason"] in ("compiler-artifact", "build-script-executed"):
            hashes |= unit_hashes(message) | copied_hashes(message)
    return process.wait()


def main(test_args):
    json_output = "--message-format=json-diagnostic-rendered-ansi"
    steps = [
        ["cargo", "fmt", "--check"],
        ["cargo", "clippy", "--all-targets", json_output, "--", "-D", "warnings"],
        ["cargo", "test", "--no-run", json_output, *test_args],
        ["cargo", "build", json_output],
    ]
    hashes = set()
    for step in steps:
        print("+", " ".join(step), flush=True)
        if code := run(step, hashes):
            print("Nothing removed: a step failed.", file=sys.stderr)
            return code
    collected = time.time()
    metadata = json.loads(
        subprocess.check_output(["cargo", "metadata", "--format-version", "1", "--no-deps"])
    )
    root = Path(metadata["workspace_root"]).resolve()
    target = Path(metadata["target_directory"]).resolve()
    if root not in target.parents:
        print(f"Nothing removed: {target} lies outside this checkout.", file=sys.stderr)
        return run(["cargo", "test", *test_args], hashes)
    debug = target / "debug"
    held = take_locks(debug)
    if held is None or written_since(debug, collected):
        print("Nothing removed: another build is using target/.", file=sys.stderr)
    else:
        freed = 0
        for folder in SWEPT:
            if (debug / folder).is_dir():
                for entry in unused(sorted(p.name for p in (debug / folder).iterdir()), hashes):
                    path = debug / folder / entry
                    freed += size(path)
                    remove(path)
        incremental = debug / "incremental"
        if size(incremental) > INCREMENTAL_LIMIT:
            freed += size(incremental)
            shutil.rmtree(incremental)
        for handle in held:
            handle.close()
        print(f"Removed {freed / 1024**3:.1f} GB. target/ is {size(target) / 1024**3:.1f} GB.")
    command = ["cargo", "test", json_output, *test_args]
    print("+", " ".join(command), flush=True)
    return run(command, hashes)


if __name__ == "__main__":
    args = sys.argv[1:]
    sys.exit(main(args[1:] if args[:1] == ["--"] else args))
