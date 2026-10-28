"""Validate the immutable engine distribution before packaging or activation."""

import hashlib
import json
from pathlib import Path, PurePosixPath
import re
import stat


def digest(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def regular(path):
    if not stat.S_ISREG(path.lstat().st_mode):
        raise ValueError("Expected a regular payload file: " + str(path))
    return path


def document(path, limit=16 * 1024 * 1024):
    regular(path)
    with path.open("rb") as stream:
        data = stream.read(limit + 1)
    if len(data) > limit:
        raise ValueError("Payload metadata exceeds its size limit")
    return json.loads(data)


def relative(name):
    if not isinstance(name, str) or not name or any(ord(c) < 32 for c in name):
        raise ValueError("Invalid payload path")
    path = PurePosixPath(name)
    if path.is_absolute() or any(part in {"", ".", ".."} for part in name.split("/")):
        raise ValueError("Unsafe payload path: " + name)
    return path


def verify(root, target, allow_private=False):
    root = Path(root)
    if not stat.S_ISDIR(root.lstat().st_mode):
        raise ValueError("The engine must be a real directory")
    # Check types before opening any inventory path. No symlink, FIFO, device
    # or socket is permitted, including a directory symlink within a path.
    actual = {}
    for path in root.rglob("*"):
        info = path.lstat()
        if not (stat.S_ISREG(info.st_mode) or stat.S_ISDIR(info.st_mode)):
            raise ValueError("Payload links and special files are unsupported")
        actual[path.relative_to(root).as_posix()] = info
    manifest = document(root / "engine-manifest.json", 64 * 1024)
    if (manifest.get("schema_version") != 1 or manifest.get("target") != target
            or manifest.get("entrypoint") != "bin/backbone"
            or manifest.get("protocols") != {"runtime_discovery": 1, "setup": 1}):
        raise ValueError("Unsupported engine manifest or target")
    identifier = manifest.get("payload_id", "")
    if (not isinstance(identifier, str) or not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9_.-]{0,127}", identifier)):
        raise ValueError("Invalid engine payload identifier")
    if manifest.get("release_ready") is not True and not allow_private:
        raise ValueError("Engine is not release-ready; private integration requires an explicit opt-in")
    for key, filename in [("files", "engine-file-inventory-v1.json"),
                          ("licenses", "engine-license-inventory.json")]:
        item = manifest.get(key, {})
        if item.get("path") != filename or digest(regular(root / filename)) != item.get("sha256"):
            raise ValueError("Engine " + key + " metadata hash mismatch")
    inventory = document(root / "engine-file-inventory-v1.json")
    if not isinstance(inventory, list) or len(inventory) > 50000:
        raise ValueError("Invalid engine inventory")
    expected = {"engine-manifest.json", "engine-file-inventory-v1.json"}
    for item in inventory:
        name = str(relative(item["path"]))
        if name in expected:
            raise ValueError("Duplicate or reserved inventory path: " + name)
        expected.add(name)
        info = actual.get(name)
        mode = item.get("mode")
        if info is None or mode not in {0o644, 0o755} or stat.S_IMODE(info.st_mode) != mode:
            raise ValueError("Engine file missing or mode changed: " + name)
        path = root / name
        if item.get("kind") == "directory":
            if not stat.S_ISDIR(info.st_mode):
                raise ValueError("Expected payload directory: " + name)
        elif item.get("kind") == "file":
            if (not stat.S_ISREG(info.st_mode) or info.st_size != item.get("size")
                    or digest(path) != item.get("sha256")):
                raise ValueError("Engine file content mismatch: " + name)
        else:
            raise ValueError("Unknown inventory entry type")
    if expected != set(actual):
        raise ValueError("Engine inventory does not cover exactly the payload contents")
    for name in ("bin/backbone", "python/bin/python3", "libexec/tmux"):
        if name not in actual or not stat.S_ISREG(actual[name].st_mode) or not actual[name].st_mode & 0o111:
            raise ValueError("Missing executable payload entrypoint: " + name)
    tmux = manifest.get("tmux", {})
    if tmux.get("path") != "libexec/tmux" or digest(root / "libexec/tmux") != tmux.get("sha256"):
        raise ValueError("Bundled tmux hash mismatch")
    return manifest
