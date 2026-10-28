"""Install a verified engine at a stable path and initialize it with replay.

Called by install.sh using the packaged Python, after archive verification.
Never removes an engine after initialization may have written service state.
"""

import argparse
import fcntl
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import stat
import subprocess
import sys
import tempfile
import time
import uuid

# The installer runs with Python isolation enabled. Import only our verified
# sibling helper, not the caller's current directory or PYTHONPATH.
sys.path.insert(0, str(Path(__file__).resolve().parent))
from engine_payload import digest, verify

OUTPUT_LIMIT = 1024 * 1024
REQUEST_LIMIT = 32 * 1024


def save_new(path, value):
    """Publish a complete private record without replacing an existing one."""
    data = (json.dumps(value, sort_keys=True) + "\n").encode()
    if len(data) > REQUEST_LIMIT:
        raise ValueError("Installation request is too large")
    with tempfile.NamedTemporaryFile(dir=path.parent) as stream:
        stream.write(data)
        stream.flush()
        os.fsync(stream.fileno())
        os.link(stream.name, path)


def read_private(path):
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    with os.fdopen(fd, "rb") as stream:
        info = os.fstat(stream.fileno())
        if not stat.S_ISREG(info.st_mode) or info.st_size > REQUEST_LIMIT:
            raise ValueError("Invalid saved installation request")
        return json.load(stream)


def save_outcome(path, record, result):
    """Keep the validated outcome bound to the exact immutable request."""
    value = {"record": record, "result": result}
    data = (json.dumps(value, sort_keys=True) + "\n").encode()
    if len(data) > REQUEST_LIMIT:
        raise ValueError("Installation outcome is too large")
    with tempfile.NamedTemporaryFile(dir=path.parent, delete=False) as stream:
        temporary = Path(stream.name)
        try:
            stream.write(data)
            stream.flush()
            os.fsync(stream.fileno())
            os.replace(temporary, path)
        finally:
            temporary.unlink(missing_ok=True)


def can_start_new(result):
    steps = result.get("completed_steps")
    return (result.get("status") == "failed" and result.get("retry") == "new_request"
            and isinstance(steps, list) and all(step == "preflight" for step in steps))


def parse_result(data, request, returncode):
    last = None
    sequence = 0
    for line in data.splitlines():
        if not line or len(line) > 64 * 1024 or last is not None:
            raise ValueError("Invalid setup output")
        event = json.loads(line)
        if (event.get("schema_version") != 1 or event.get("request_id") != request["request_id"]
                or type(event.get("seq")) is not int or event["seq"] <= sequence
                or event["seq"] > 1024):
            raise ValueError("Setup output does not match the saved request")
        sequence = event["seq"]
        if event.get("type") == "result":
            last = event
        elif event.get("type") != "step":
            raise ValueError("Unknown setup event")
    if last is None or last.get("action") != "initialize":
        raise ValueError("Setup did not report a final initialization result")
    expected = {"completed": {0}, "action_required": {3}, "failed": {1, 2}}
    if returncode not in expected.get(last.get("status"), set()):
        raise ValueError("Setup result and exit status disagree")
    if last["status"] != "completed":
        if (last.get("retry") not in {"same_request", "new_request", "none"}
                or not isinstance(last.get("completed_steps"), list)
                or any(not isinstance(step, str) for step in last["completed_steps"])):
            raise ValueError("Invalid initialization recovery state")
        return last
    if last.get("code") not in {"initialized", "existing_preserved"}:
        raise ValueError("Unrecognized initialization completion")
    result = last.get("data", {})
    if result.get("data_dir") != request["data_dir"]:
        raise ValueError("Initialization returned a different data directory")
    installation = "created" if last["code"] == "initialized" else "existing_preserved"
    if result.get("installation") != installation:
        raise ValueError("Initialization returned an inconsistent installation state")
    if (installation == "created" and request["service"] == "user"
            and result.get("health") != "healthy"):
        raise ValueError("The new engine service is not confirmed healthy")
    return last


def initialize(executable, request, path_env, timeout=300):
    # Preserve inherited authority. Only bytecode policy and the original PATH
    # are supplied; no agent key is discarded or replaced with operator access.
    env = dict(os.environ, PATH=path_env, PYTHONDONTWRITEBYTECODE="1")
    with tempfile.NamedTemporaryFile() as source, tempfile.TemporaryFile() as output:
        source.write(json.dumps(request).encode())
        source.flush()
        process = subprocess.Popen([str(executable), "setup", "--file", source.name, "--jsonl"],
                                   env=env, stdin=subprocess.DEVNULL, stdout=output,
                                   stderr=subprocess.DEVNULL)
        deadline = time.monotonic() + timeout
        try:
            while process.poll() is None:
                if time.monotonic() > deadline or os.fstat(output.fileno()).st_size > OUTPUT_LIMIT:
                    raise ValueError("Setup outcome is unknown; retry the same install to resume its saved request")
                time.sleep(0.05)
            if os.fstat(output.fileno()).st_size > OUTPUT_LIMIT:
                raise ValueError("Setup output exceeded its limit")
            data = os.pread(output.fileno(), OUTPUT_LIMIT + 1, 0)
            return parse_result(data, request, process.returncode)
        finally:
            if process.poll() is None:
                process.kill()
            process.wait()


def install(source, target, home, data_dir, service):
    manifest = verify(source, target, allow_private=True)
    root = home / ".local/lib/agent-interface-engine"
    root.mkdir(parents=True, exist_ok=True, mode=0o700)
    if not stat.S_ISDIR(root.lstat().st_mode):
        raise ValueError("Engine installation root must be a real directory")
    fd = os.open(root / ".install.lock", os.O_RDWR | os.O_CREAT | os.O_NOFOLLOW | os.O_NONBLOCK, 0o600)
    with os.fdopen(fd, "rb") as lock:
        if not stat.S_ISREG(os.fstat(lock.fileno()).st_mode):
            raise ValueError("Engine lock is not a regular file")
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        try:
            return install_locked(source, target, root, data_dir, service, manifest)
        finally:
            fcntl.flock(lock, fcntl.LOCK_UN)


def install_locked(source, target, root, data_dir, service, manifest):
    final = root / manifest["payload_id"]
    if final.exists() or final.is_symlink():
        verify(final, target, allow_private=True)
        if digest(final / "engine-manifest.json") != digest(source / "engine-manifest.json"):
            raise ValueError("An engine with this identifier has different contents")
    else:
        stage = Path(tempfile.mkdtemp(prefix=".stage-", dir=root))
        try:
            shutil.copytree(source, stage / "payload")
            verify(stage / "payload", target, allow_private=True)
            (stage / "payload").rename(final)
        finally:
            shutil.rmtree(stage)
    identity = hashlib.sha256(os.fsencode(str(data_dir))).hexdigest()
    record_path = root / ("initialize-" + identity + ".json")
    outcome_path = root / ("initialize-" + identity + ".result.json")
    outcome = {}
    if record_path.exists() and outcome_path.exists():
        previous = read_private(record_path)
        saved = read_private(outcome_path)
        if saved.get("record") == previous:
            outcome = saved.get("result", {})
            if can_start_new(outcome):
                # A confirmed failure before mutation permits new choices.
                # Keep both prior records for diagnosis; never discard an
                # interrupted operation or a result with completed mutations.
                archive = root / ("previous-initialize-" + uuid.uuid4().hex)
                record_path.rename(archive.with_suffix(".json"))
                outcome_path.rename(archive.with_suffix(".result.json"))
                outcome = {}
    if not record_path.exists() and not record_path.is_symlink():
        record = {
            "schema_version": 1, "payload_id": manifest["payload_id"],
            "manifest_sha256": digest(final / "engine-manifest.json"),
            "path_env": os.environ.get("PATH", "/usr/bin:/bin"),
            "request": {"schema_version": 1, "request_id": uuid.uuid4().hex,
                        "action": "initialize", "data_dir": str(data_dir),
                        "service": service, "starter_profile": "core-v1"},
        }
        save_new(record_path, record)
    record = read_private(record_path)
    identifier = record.get("payload_id", "")
    if not isinstance(identifier, str) or not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9_.-]{0,127}", identifier):
        raise ValueError("Invalid saved engine identifier")
    request = record.get("request", {})
    if (record.get("schema_version") != 1 or request.get("schema_version") != 1
            or request.get("action") != "initialize" or request.get("data_dir") != str(data_dir)
            or request.get("service") not in {"none", "user"}
            or (request["service"] != service and outcome.get("status") != "completed")
            or request.get("starter_profile") != "core-v1"
            or not isinstance(request.get("request_id"), str) or not request["request_id"]
            or not isinstance(record.get("path_env"), str)):
        raise ValueError("Saved installation choices differ; the original request was retained")
    # A retry always uses the original immutable launcher, even when a newer
    # archive was supplied after interruption.
    engine = root / identifier
    verify(engine, target, allow_private=True)
    if digest(engine / "engine-manifest.json") != record.get("manifest_sha256"):
        raise ValueError("Saved engine identity changed")
    result = initialize(engine / "bin/backbone", request, record["path_env"])
    if result["status"] != "completed":
        recovery = {key: result[key] for key in ("status", "retry", "completed_steps")}
        save_outcome(outcome_path, record, recovery)
        # Never echo arbitrary provider output, which may contain account context.
        if can_start_new(recovery):
            raise ValueError("Engine preflight failed before changes were made. Resolve the provider issue and rerun the installer; new installation choices are allowed.")
        raise ValueError("Engine initialization needs attention; the saved request and payload were retained. Retry the same install after resolving the provider issue.")
    if result["code"] == "existing_preserved":
        save_outcome(outcome_path, record, {"status": "completed"})
        return "Existing engine configuration and service preserved."
    activation = {"schema_version": 1, "payload_id": identifier,
                  "executable": str(engine / "bin/backbone"), "data_dir": str(data_dir)}
    active = root / "active.json"
    try:
        save_new(active, activation)
    except FileExistsError:
        if read_private(active) != activation:
            raise ValueError("An existing engine activation was preserved; the new payload and request were retained")
    save_outcome(outcome_path, record, {"status": "completed"})
    return "Engine initialized; native setup can create your first agent."


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", required=True, type=Path)
    parser.add_argument("--target", required=True)
    parser.add_argument("--service", choices=["user", "none"], default="user")
    args = parser.parse_args()
    try:
        home = Path(os.environ["HOME"])
        if not home.is_absolute():
            raise ValueError("HOME must be absolute")
        selected = os.environ.get("BACKBONE_DATA_DIR") or str(home / ".local/share/agent-backbone")
        data_dir = Path(selected).expanduser().resolve()
        print(install(args.source, args.target, home, data_dir, args.service))
    except (OSError, ValueError, KeyError, TypeError) as error:
        raise SystemExit("Agent Interface engine: " + str(error)) from None
