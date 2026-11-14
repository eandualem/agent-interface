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

import json
import re
import shutil
import subprocess
import sys
from pathlib import Path

# A unit's metadata hash, as cargo writes it after the unit's name.
HASH = re.compile(r"-([0-9a-f]{16})(?=\.|$)")
INCREMENTAL_LIMIT = 2 * 1024**3
SWEPT = ("deps", ".fingerprint", "build", "examples")


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


def unit_names(message):
    """Names to keep for an artifact cargo copied out of deps/ without its
    hash, such as the app's binary, whose hash cargo does not report."""
    if message.get("reason") != "compiler-artifact" or unit_hashes(message):
        return set()
    target = message.get("target", {}).get("name", "")
    package = message.get("package_id", "").rsplit("#", 1)[-1].split("@")[0]
    names = {target, target.replace("-", "_"), package}
    return {name for name in names if name}


def unused(entries, hashes, names):
    """The entries no unit of the latest build owns."""
    for entry in entries:
        match = HASH.search(entry)
        if match and match.group(1) not in hashes and entry[: match.start()] not in names:
            yield entry


def size(path):
    if path.is_symlink() or path.is_file():
        return path.lstat().st_size
    return sum(size(child) for child in path.iterdir()) if path.is_dir() else 0


def remove(path):
    if path.is_dir() and not path.is_symlink():
        shutil.rmtree(path)
    else:
        path.unlink()


def run(command, hashes, names):
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
            hashes |= unit_hashes(message)
            names |= unit_names(message)
    return process.wait()


def main(test_args):
    json_output = "--message-format=json-diagnostic-rendered-ansi"
    steps = [
        ["cargo", "fmt", "--check"],
        ["cargo", "clippy", "--all-targets", json_output, "--", "-D", "warnings"],
        ["cargo", "test", "--no-run", json_output, *test_args],
        ["cargo", "build", json_output],
    ]
    hashes, names = set(), set()
    for step in steps:
        print("+", " ".join(step), flush=True)
        if code := run(step, hashes, names):
            print("Nothing removed: a step failed.", file=sys.stderr)
            return code
    metadata = json.loads(
        subprocess.check_output(["cargo", "metadata", "--format-version", "1", "--no-deps"])
    )
    root = Path(metadata["workspace_root"]).resolve()
    target = Path(metadata["target_directory"]).resolve()
    if root not in target.parents:
        print(f"Nothing removed: {target} lies outside this checkout.", file=sys.stderr)
        return run(["cargo", "test", *test_args], hashes, names)
    debug = target / "debug"
    freed = 0
    for folder in SWEPT:
        if (debug / folder).is_dir():
            for entry in unused(sorted(p.name for p in (debug / folder).iterdir()), hashes, names):
                path = debug / folder / entry
                freed += size(path)
                remove(path)
    incremental = debug / "incremental"
    if size(incremental) > INCREMENTAL_LIMIT:
        freed += size(incremental)
        shutil.rmtree(incremental)
    print(f"Removed {freed / 1024**3:.1f} GB. target/ is {size(target) / 1024**3:.1f} GB.")
    command = ["cargo", "test", json_output, *test_args]
    print("+", " ".join(command), flush=True)
    return run(command, hashes, names)


if __name__ == "__main__":
    args = sys.argv[1:]
    sys.exit(main(args[1:] if args[:1] == ["--"] else args))
