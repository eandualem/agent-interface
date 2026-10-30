"""Guard Linux package changes; never restart services or touch tmux sessions."""

import argparse
import asyncio
import hashlib
import importlib.util
from importlib.metadata import version as package_version
import json
import os
from pathlib import Path
import re
import socket
import stat
import struct
import time
import uuid

SPEC = importlib.util.spec_from_file_location("setup_backbone", Path(__file__).with_name("setup_backbone.py"))
setup = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(setup)

DEFERRED = ("Backbone upgrade deferred before changing packages. Existing Linux services must "
            "already be running Backbone 0.4.3 or newer from a verified fixed launch. "
            "Versions 0.4.0–0.4.2 may have passed listener descriptors to tmux children; "
            "installing new code does not repair those children. Continue existing work, "
            "then coordinate session completion and recovery through Backbone. "
            "This installer will not stop sessions or restart the service.")


def fixed_version(value):
    return isinstance(value, str) and re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+", value) is not None and tuple(map(int, value.split("."))) >= (0, 4, 3)


def peer_process(client):
    pid, uid, _ = struct.unpack("iII", client.getsockopt(socket.SOL_SOCKET, socket.SO_PEERCRED, struct.calcsize("iII")))
    if uid != os.getuid() or pid <= 0:
        raise ValueError("Private endpoint peer could not be verified")
    started = Path(f"/proc/{pid}/stat").read_text().rsplit(") ", 1)[1].split()[19]
    return [pid, started]


def exchange(path, method, route, token="", payload=None, expected_process=None):
    """Bounded HTTP over a private, same-user socket; never use a proxy."""
    directory, endpoint = path.parent.lstat(), path.lstat()
    if (not stat.S_ISDIR(directory.st_mode) or directory.st_uid != os.getuid()
            or stat.S_IMODE(directory.st_mode) != 0o700
            or not stat.S_ISSOCK(endpoint.st_mode) or endpoint.st_uid != os.getuid()
            or stat.S_IMODE(endpoint.st_mode) != 0o600):
        raise ValueError("Private endpoint ownership could not be verified")
    if any(ord(c) < 32 or ord(c) > 126 for c in token):
        raise ValueError("Invalid installation credential")
    deadline = time.monotonic() + 5
    with socket.socket(socket.AF_UNIX) as client:
        client.settimeout(5)
        client.connect(str(path))
        # PID alone may be reused. exec retains both PID and process start time.
        process = peer_process(client)
        if expected_process is not None and process != expected_process:
            raise ValueError("Backbone process changed; upgrade hold was retained")
        body = json.dumps(payload).encode() if payload is not None else b""
        headers = f"{method} {route} HTTP/1.0\r\nHost: localhost\r\nConnection: close\r\n"
        if token:
            headers += f"Authorization: Bearer {token}\r\n"
        headers += f"Content-Type: application/json\r\nContent-Length: {len(body)}\r\n\r\n"
        client.sendall(headers.encode() + body)
        response = b""
        while len(response) <= 65536:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise ValueError("Upgrade coordination timed out")
            client.settimeout(remaining)
            part = client.recv(min(4096, 65537 - len(response)))
            if not part:
                break
            response += part
        else:
            raise ValueError("Upgrade coordination response exceeded its limit")
    header, separator, body = response.partition(b"\r\n\r\n")
    if not separator or header.split(b"\r\n", 1)[0] not in (b"HTTP/1.0 200 OK", b"HTTP/1.1 200 OK"):
        raise ValueError("Backbone refused upgrade coordination; no automatic recovery was attempted")
    result = json.loads(body)
    if not isinstance(result, dict):
        raise ValueError("Invalid upgrade coordination response")
    return result, process


def operator_token(data):
    # Provider-owned, read-only credential loading preserves its authority rules.
    # Never replace an inherited agent credential with operator authority.
    from agent_backbone.cli._common import read_client_config

    config = asyncio.run(read_client_config(data, strict=True, operator_file=True))
    if not config.api_key:
        raise ValueError("Operator authentication is unavailable")
    return config.api_key


def owned_state(home, data, executable):
    root = home / ".local/state/agent-interface/backbone"
    info = root.lstat()
    if (not stat.S_ISDIR(info.st_mode) or info.st_uid != os.getuid()
            or stat.S_IMODE(info.st_mode) != 0o700):
        raise ValueError(DEFERRED)
    identity = hashlib.sha256(os.fsencode(str(data))).hexdigest()
    record = setup.read_private(root / (identity + ".json"))
    outcome = setup.read_private(root / (identity + ".result.json"))
    state = outcome.get("result", {})
    env = dict(os.environ, BACKBONE_DATA_DIR=str(data))
    if (outcome.get("record") != record or state.get("status") != "completed"
            or state.get("installation_identity") != setup.installation_identity(data)
            or state.get("service_identity") != setup.service_identity(home, data, executable, env)
            or not state.get("socket") or state["socket"] != setup.socket_setting(executable, env)):
        raise ValueError(DEFERRED)
    state["main_pid"] = int(setup.command("systemctl", ["--user", "show", "agent-backbone.service",
                                                      "--property=MainPID", "--value"], env).strip())
    if state["main_pid"] <= 0:
        raise ValueError(DEFERRED)
    return root / (identity + ".upgrade.json"), state


def persist(path, record, released=False):
    setup.save_outcome(path, record, {"released": released})
    fd = os.open(path.parent, os.O_RDONLY | os.O_DIRECTORY)
    try:
        os.fsync(fd)
    finally:
        os.close(fd)


def guard(home, data, executable, phase):
    path, state = owned_state(home, data, executable)
    endpoint = Path(state["socket"])
    health, process = exchange(endpoint, "GET", "/health")
    if process[0] != state["main_pid"]:
        raise ValueError(DEFERRED)
    if health.get("healthy") is not True or not fixed_version(health.get("version")):
        raise ValueError(DEFERRED)
    if not fixed_version(package_version("agent-backbone")):
        raise ValueError(DEFERRED)
    token = operator_token(data)
    # Require the authenticated request to reach the same process as preflight.
    health, _ = exchange(endpoint, "GET", "/health", token, expected_process=process)
    if health.get("healthy") is not True or not fixed_version(health.get("version")):
        raise ValueError(DEFERRED)
    if type(health.get("started")) not in (int, float) or not 0 < health["started"] < float("inf"):
        raise ValueError(DEFERRED)
    saved = setup.read_private(path) if path.exists() or path.is_symlink() else None
    expected = {"process": process, "installation_identity": state["installation_identity"],
                "service_identity": state["service_identity"], "socket": str(endpoint),
                "started": health["started"], "running_version": health["version"]}
    if phase == "verify":
        if not saved or saved.get("result", {}).get("released") is not True:
            raise ValueError("No released upgrade operation to verify")
        record = saved["record"]
        if (any(record.get(k) != expected[k] for k in ("process", "installation_identity", "service_identity", "socket"))
                or health["version"] != package_version("agent-backbone")):
            raise ValueError("Upgraded service identity or version could not be verified")
        print("Updated Backbone is healthy on the same service process.")
        return
    if saved and saved.get("result", {}).get("released") is False:
        record = saved["record"]
        if any(record.get(k) != v for k, v in expected.items()):
            raise ValueError("Saved upgrade ownership changed; hold and package were preserved")
        if str(uuid.UUID(record["operation_id"])) != record["operation_id"]:
            raise ValueError("Invalid saved upgrade operation")
    elif phase == "before":
        record = dict(expected, operation_id=str(uuid.uuid4()))
    else:
        raise ValueError("No pending upgrade hold; service was preserved")
    if phase == "before":
        # Durable operation ID precedes even a lost acknowledgement.
        persist(path, record)
    else:
        setup.check_schema(executable, dict(os.environ, BACKBONE_DATA_DIR=str(data)))
    enabled = phase == "before"
    result, _ = exchange(endpoint, "POST", "/api/upgrade/hold", token,
                         {"operation_id": record["operation_id"], "enabled": enabled}, process)
    if result.get("operation_held") is not enabled or type(result.get("held")) is not bool:
        raise ValueError("Upgrade hold acknowledgement is uncertain; saved state was preserved")
    if enabled and result["held"] is not True:
        raise ValueError("Automatic upgrade restart could not be held")
    if enabled:
        _, current = owned_state(home, data, executable)
        health, _ = exchange(endpoint, "GET", "/health", token, expected_process=process)
        if (current != state or health.get("started") != record["started"]
                or health.get("version") != record["running_version"]
                or health.get("healthy") is not True):
            raise ValueError("Backbone changed during preflight; hold was retained")
    if not enabled:
        persist(path, record, released=True)
    print("Automatic reload held for package update." if enabled else
          "Package update complete; this installer's reload hold released. Other holds and settings are unchanged.")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--executable", type=Path, required=True)
    parser.add_argument("--phase", choices=("before", "after", "verify"), required=True)
    args = parser.parse_args()
    try:
        home = Path(os.environ["HOME"])
        if not home.is_absolute() or not args.executable.is_absolute():
            raise ValueError("Installation paths must be absolute")
        data = Path(os.environ.get("BACKBONE_DATA_DIR") or home / ".local/share/agent-backbone").expanduser().resolve()
        guard(home, data, args.executable, args.phase)
    except (OSError, ValueError, KeyError, TypeError, IndexError):
        # Never render credentials, server response bodies or provider exceptions.
        raise SystemExit("Agent Interface: " + (DEFERRED if args.phase == "before" else
                         "Upgrade coordination is incomplete; the saved operation was retained. Resolve through Backbone. "
                         "No service restart was attempted.")) from None
