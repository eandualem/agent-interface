"""Initialize the published Backbone installation using its finite setup API.

The app installer supplies the uv-installed executable and Python. Backbone
owns its distribution, configuration and service; this helper retains the exact
initialization request so an interrupted install can be retried.
"""

import argparse
import fcntl
import hashlib
import json
import os
from pathlib import Path
import stat
import subprocess
import tempfile
import time
import uuid

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
        if (not stat.S_ISREG(info.st_mode) or info.st_size > REQUEST_LIMIT
                or info.st_uid != os.getuid() or stat.S_IMODE(info.st_mode) & 0o077):
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


def command(executable, arguments, env):
    with tempfile.TemporaryFile() as output:
        process = subprocess.Popen([str(executable), *arguments], env=env,
                                   stdin=subprocess.DEVNULL, stdout=output, stderr=subprocess.DEVNULL)
        deadline = time.monotonic() + 60
        try:
            while process.poll() is None:
                if time.monotonic() > deadline or os.fstat(output.fileno()).st_size > OUTPUT_LIMIT:
                    raise ValueError("Backbone did not finish the setup command; progress was retained")
                time.sleep(0.05)
            if process.returncode or os.fstat(output.fileno()).st_size > OUTPUT_LIMIT:
                raise ValueError("Backbone refused the setup command; progress was retained")
            return os.pread(output.fileno(), OUTPUT_LIMIT + 1, 0)
        finally:
            if process.poll() is None:
                process.kill()
            process.wait()


def check_schema(executable, env):
    schema = json.loads(command(executable, ["setup", "--schema"], env))
    definitions = schema.get("$defs", {})
    for name, action in (("SetupInitialize", "initialize"), ("SetupFirstAgent", "first_agent")):
        properties = definitions.get(name, {}).get("properties", {})
        if (properties.get("schema_version", {}).get("const") != 1
                or properties.get("action", {}).get("const") != action):
            raise ValueError("The published Backbone does not support this app's setup protocol")


def service_identity(home, data_dir, executable, env):
    """Check both the on-disk definition and the loaded user-service identity."""
    import platform
    import plistlib
    import shlex

    mac = platform.system() == "Darwin"
    path = home / ("Library/LaunchAgents/dev.agent-backbone.plist" if mac
                   else ".config/systemd/user/agent-backbone.service")
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    with os.fdopen(fd, "rb") as stream:
        info = os.fstat(stream.fileno())
        if not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid() or info.st_size > 64 * 1024:
            raise ValueError("The Backbone service definition changed; it was preserved")
        content = stream.read()
    if mac:
        definition = plistlib.loads(content)
        owner = definition.get("EnvironmentVariables", {}).get("BACKBONE_DATA_DIR")
        arguments = definition.get("ProgramArguments", [])
        loaded = command("launchctl", ["print", f"gui/{os.getuid()}/dev.agent-backbone"], env).decode()
        owners = [line.partition("=>")[2].strip() for line in loaded.splitlines()
                  if line.strip().startswith("BACKBONE_DATA_DIR =>")]
    else:
        lines = content.decode().splitlines()
        values = [item for line in lines if line.startswith("Environment=")
                  for item in shlex.split(line.partition("=")[2].replace("%%", "%"))]
        owners_on_disk = [item.partition("=")[2] for item in values if item.startswith("BACKBONE_DATA_DIR=")]
        owner = owners_on_disk[0] if len(owners_on_disk) == 1 else None
        starts = [line.partition("=")[2].replace("%%", "%") for line in lines if line.startswith("ExecStart=")]
        arguments = shlex.split(starts[0]) if len(starts) == 1 else []
        loaded = command("systemctl", ["--user", "show", "agent-backbone.service",
                                      "--property=Environment", "--value"], env).decode()
        owners = [item.partition("=")[2] for item in shlex.split(loaded)
                  if item.startswith("BACKBONE_DATA_DIR=")]
    if (owner != str(data_dir) or owners != [str(data_dir)] or not arguments
            or Path(arguments[0]).resolve() != executable.resolve()):
        raise ValueError("The Backbone service belongs to a different installation; it was preserved")
    return hashlib.sha256(content).hexdigest()


def socket_setting(executable, env):
    output = command(executable, ["config", "get", "backbone.socket_path"], env)
    value = json.loads(output.splitlines()[0])
    if not isinstance(value, str):
        raise ValueError("Backbone returned an invalid socket setting")
    return value


def installation_identity(data_dir):
    credentials = data_dir / ".env"
    fd = os.open(credentials, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    with os.fdopen(fd, "rb") as stream:
        info = os.fstat(stream.fileno())
        if (not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid()
                or info.st_size > 64 * 1024 or stat.S_IMODE(info.st_mode) & 0o077):
            raise ValueError("The installation credentials changed; they were preserved")
        digest = hashlib.sha256(stream.read()).hexdigest()
    database = (data_dir / "backbone.db").lstat()
    if not stat.S_ISREG(database.st_mode) or database.st_uid != os.getuid():
        raise ValueError("The installation database changed; it was preserved")
    return [digest, database.st_dev, database.st_ino]


def socket_healthy(path):
    """Probe only public health data. Never read or transmit a credential."""
    import socket

    try:
        directory, endpoint = path.parent.lstat(), path.lstat()
        if (not stat.S_ISDIR(directory.st_mode) or directory.st_uid != os.getuid()
                or stat.S_IMODE(directory.st_mode) != 0o700
                or not stat.S_ISSOCK(endpoint.st_mode) or endpoint.st_uid != os.getuid()
                or stat.S_IMODE(endpoint.st_mode) != 0o600):
            return False
        with socket.socket(socket.AF_UNIX) as client:
            client.settimeout(2)
            client.connect(str(path))
            client.sendall(b"GET /health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            response = b""
            while b"\r\n" not in response and len(response) < 4096:
                part = client.recv(4096 - len(response))
                if not part:
                    break
                response += part
            return response.split(b"\r\n", 1)[0] in (b"HTTP/1.1 200 OK", b"HTTP/1.0 200 OK")
    except OSError:
        return False


def finish_socket(executable, home, data_dir, record, state, outcome_path, env):
    if installation_identity(data_dir) != state["installation_identity"]:
        raise ValueError("The installation changed during setup; it was preserved")
    identity = service_identity(home, data_dir, executable, env)
    current = socket_setting(executable, env)
    if "socket" not in state:
        if current:
            raise ValueError("A socket choice appeared during setup; it was preserved")
        state.update(socket=str(data_dir / "run/backbone.sock"), service_identity=identity)
        save_outcome(outcome_path, record, state)
    if identity != state["service_identity"] or current not in ("", state["socket"]):
        raise ValueError("The service or socket choice changed during setup; it was preserved")
    if state.get("socket_configured") and current != state["socket"]:
        raise ValueError("The saved socket choice changed; it was preserved")
    path = Path(state["socket"])
    path.parent.mkdir(mode=0o700, exist_ok=True)
    parent = path.parent.lstat()
    if (not stat.S_ISDIR(parent.st_mode) or parent.st_uid != os.getuid()
            or stat.S_IMODE(parent.st_mode) != 0o700):
        raise ValueError("The socket directory must be private and owned by this user")
    if current != state["socket"]:
        command(executable, ["config", "set", "backbone.socket_path", state["socket"]], env)
    if socket_setting(executable, env) != state["socket"]:
        raise ValueError("The socket setting could not be confirmed")
    state["socket_configured"] = True
    save_outcome(outcome_path, record, state)
    if not socket_healthy(path):
        if service_identity(home, data_dir, executable, env) != state["service_identity"]:
            raise ValueError("The service changed before restart; it was preserved")
        command(executable, ["service", "restart"], env)
        for _ in range(20):
            if socket_healthy(path):
                break
            time.sleep(0.5)
        else:
            raise ValueError("The private Backbone socket is not healthy; rerun the installer to retry")
    state["status"] = "completed"
    save_outcome(outcome_path, record, state)


def install(executable, home, data_dir, version, provider_identity, resume_only=False):
    root = home / ".local/state/agent-interface/backbone"
    root.mkdir(mode=0o700, parents=True, exist_ok=True)
    root_info = root.lstat()
    if (not stat.S_ISDIR(root_info.st_mode) or root_info.st_uid != os.getuid()
            or stat.S_IMODE(root_info.st_mode) != 0o700):
        raise ValueError("The saved setup directory must be private and owned by this user")
    fd = os.open(root / ".lock", os.O_RDWR | os.O_CREAT | os.O_NOFOLLOW | os.O_NONBLOCK, 0o600)
    with os.fdopen(fd, "rb") as lock:
        if not stat.S_ISREG(os.fstat(lock.fileno()).st_mode):
            raise ValueError("The setup lock is not a regular file")
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        identity = hashlib.sha256(os.fsencode(str(data_dir))).hexdigest()
        request_path = root / (identity + ".json")
        outcome_path = root / (identity + ".result.json")
        state = {}
        record = read_private(request_path) if request_path.exists() or request_path.is_symlink() else None
        if record and (outcome_path.exists() or outcome_path.is_symlink()):
            saved = read_private(outcome_path)
            if saved.get("record") != record:
                raise ValueError("The saved setup outcome does not match its request")
            state = saved.get("result", {})
        if state.get("status") == "completed":
            if not resume_only:
                check_schema(executable, dict(os.environ, BACKBONE_DATA_DIR=str(data_dir)))
            return "Existing Backbone configuration and service preserved.", False
        if can_start_new(state):
            archive = root / ("previous-" + uuid.uuid4().hex)
            request_path.rename(archive.with_suffix(".json"))
            outcome_path.rename(archive.with_suffix(".result.json"))
            record, state = None, {}
        if record is None and resume_only:
            return "No interrupted Backbone setup.", False
        if record is None:
            record = {"version": version, "provider_identity": provider_identity, "executable": str(executable),
                      "path_env": os.environ.get("PATH", "/usr/bin:/bin"),
                      "request": {"schema_version": 1, "request_id": uuid.uuid4().hex,
                                  "action": "initialize", "data_dir": str(data_dir),
                                  "service": "user", "starter_profile": "core-v1"}}
            save_new(request_path, record)
        request = record.get("request", {})
        if (record.get("version") != version or record.get("provider_identity") != provider_identity
                or record.get("executable") != str(executable)
                or request.get("data_dir") != str(data_dir) or request.get("action") != "initialize"
                or request.get("service") != "user" or request.get("schema_version") != 1
                or request.get("starter_profile") != "core-v1"
                or not isinstance(request.get("request_id"), str)
                or not isinstance(record.get("path_env"), str)):
            raise ValueError(f"The installation changed during setup (saved Backbone {record.get('version')}, installed {version}, step {state.get('status', 'initialize')}); its original request was retained")
        env = dict(os.environ, BACKBONE_DATA_DIR=str(data_dir), PATH=record["path_env"], PYTHONDONTWRITEBYTECODE="1")
        check_schema(executable, env)
        if state.get("status") != "initialized":
            result = initialize(executable, request, record["path_env"])
            if result["status"] != "completed":
                save_outcome(outcome_path, record, result)
                raise ValueError("Backbone initialization needs attention; rerun the installer to resume")
            if result["code"] == "existing_preserved":
                save_outcome(outcome_path, record, {"status": "completed"})
                return "Existing Backbone configuration and service preserved.", resume_only
            state = {"status": "initialized", "installation_identity": installation_identity(data_dir)}
            save_outcome(outcome_path, record, state)
        finish_socket(executable, home, data_dir, record, state, outcome_path, env)
        return "Agent Backbone is ready for native onboarding.", resume_only


if __name__ == "__main__":
    from importlib.metadata import distributions, version as package_version

    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--executable", required=True, type=Path)
    parser.add_argument("--resume-pending", action="store_true")
    args = parser.parse_args()
    try:
        home = Path(os.environ["HOME"])
        if not home.is_absolute() or not args.executable.is_absolute():
            raise ValueError("Installation paths must be absolute")
        data = Path(os.environ.get("BACKBONE_DATA_DIR") or home / ".local/share/agent-backbone").expanduser().resolve()
        # Tool upgrades replace distribution metadata, including dependency
        # versions. Bind pending setup to that installation and its launcher.
        metadata = sorted((distribution.metadata["Name"], distribution.version,
                           distribution.read_text("RECORD") or "") for distribution in distributions())
        identity = hashlib.sha256(json.dumps(metadata).encode() + args.executable.read_bytes()).hexdigest()
        message, resumed = install(args.executable, home, data, package_version("agent-backbone"), identity, args.resume_pending)
        print(message)
        if resumed:
            raise SystemExit(3)
    except (OSError, ValueError, KeyError, TypeError) as error:
        raise SystemExit("Agent Interface setup: " + str(error)) from None
