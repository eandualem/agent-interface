#!/usr/bin/env python3
"""A stand-in Agent Backbone for the Linux first-run regression: the `backbone`
CLI and its profile sockets, speaking the profile contract the app's Rust
clients expect (crates/roster/tests/fixtures/profiles-v1.json).

Three roles, one file:

  fake_backbone.py init ACCOUNTS    a registry with a ready Default profile
  fake_backbone.py serve            the socket servers (one per running profile)
  fake_backbone.py <backbone args>  the CLI: profiles list/show/apply/operation,
                                    runtimes, setup

ACCOUNTS is a JSON object of runtime id to authentication, such as
{"claude": "authenticated"}; new profiles start with the same accounts.

Everything lives under FAKE_BACKBONE_ROOT (the test's temporary HOME):
  provider/                 BACKBONE_DATA_DIR of the Default (legacy) profile
  provider/profiles/<id>/   each new profile's .env, run/api.sock and tmux/
  fake/state.json           registry, operations, agents, accounts
  fake/*.jsonl              logs of CLI calls, HTTP requests and setup requests

Nothing here starts tmux, an agent CLI or a network connection.
"""
import fcntl
import http.server
import json
import os
import secrets
import socketserver
import sys
import threading
import time
import urllib.parse
from contextlib import contextmanager
from pathlib import Path

ROOT = Path(os.environ.get("FAKE_BACKBONE_ROOT") or os.environ["HOME"])
FAKE = ROOT / "fake"
STATE = FAKE / "state.json"
LOCK = FAKE / "state.lock"
BASELINE = "core-v1"
# FAKE_BACKBONE_SLOW=1 adds delays so each busy state stays on screen long
# enough to watch or record.
SLOW = os.environ.get("FAKE_BACKBONE_SLOW") == "1"
CREATE_DELAY = 1.4 if SLOW else 0.0
START_DELAY = 0.9 if SLOW else 0.0
STEP_DELAY = 0.75 if SLOW else 0.0

RUNTIMES = [
    # id, display name, installed
    ("claude", "Claude Code", True),
    ("codex", "Codex", True),
    ("gemini", "Gemini CLI", False),
    ("opencode", "OpenCode", False),
    ("shell", "Plain shell", True),
]
VERSIONS = {"claude": "2.1.0", "codex": "0.160.0"}


# ---------------------------------------------------------------- state ----

def now():
    return time.time()


def private_dir(path):
    path.mkdir(parents=True, exist_ok=True)
    path.chmod(0o700)
    return path


@contextmanager
def locked(write=True):
    private_dir(FAKE)
    with open(LOCK, "a+") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX if write else fcntl.LOCK_SH)
        state = json.loads(STATE.read_text()) if STATE.exists() else None
        holder = {"state": state}
        yield holder
        if write and holder["state"] is not None:
            tmp = STATE.with_suffix(".tmp")
            tmp.write_text(json.dumps(holder["state"], indent=2))
            os.replace(tmp, STATE)


def log(name, record):
    private_dir(FAKE)
    record = {"t": round(now(), 3), **record}
    with open(FAKE / name, "a") as f:
        f.write(json.dumps(record) + "\n")


def profile_paths(profile_id, legacy):
    base = ROOT / "provider" if legacy else ROOT / "provider" / "profiles" / profile_id
    return {
        "base": base,
        "socket": base / "run" / "api.sock",
        "credential": base / ".env",
        "tmux": base / "tmux" / "server.sock",
    }


def provision(profile_id, legacy, key):
    paths = profile_paths(profile_id, legacy)
    private_dir(paths["base"])
    private_dir(paths["base"] / "run")
    private_dir(paths["base"] / "tmux")
    paths["credential"].write_text(f"BACKBONE_API_KEY={key}\n")
    paths["credential"].chmod(0o600)
    return paths


def new_profile(profile_id, display_name, kind, accounts):
    legacy = kind == "legacy_default"
    key = "key-" + secrets.token_hex(12)
    paths = provision(profile_id, legacy, key)
    return {
        "profile_id": profile_id,
        "display_name": display_name,
        "kind": kind,
        "state": "creating",
        "installation_id": f"installation-{profile_id}",
        "lifecycle_id": f"lifecycle-{profile_id}-1",
        "key": key,
        "running": False,
        "socket": str(paths["socket"]),
        "credential": str(paths["credential"]),
        "tmux": str(paths["tmux"]),
        "agents": [],
        "accounts": dict(accounts),
    }


def init(accounts):
    """Fresh registry holding a ready, running Default profile with no agents."""
    private_dir(FAKE)
    with locked() as holder:
        default = new_profile("default", "Default", "legacy_default", accounts)
        default["state"] = "ready"
        default["running"] = True
        # The legacy roster reads BACKBONE_DATA_DIR/backbone.db and run/api.sock.
        (ROOT / "provider" / "backbone.db").touch()
        holder["state"] = {
            "profiles": [default],
            "operations": {},
            "setups": {},
            "new_accounts": dict(accounts),
        }
    return default


def find(state, profile_id):
    return next((p for p in state["profiles"] if p["profile_id"] == profile_id), None)


def identity(profile):
    return {
        "profile_id": profile["profile_id"],
        "installation_id": profile["installation_id"],
        "lifecycle_id": profile["lifecycle_id"],
    }


def item(profile):
    return {k: profile[k] for k in ("profile_id", "display_name", "kind", "state")}


def record(profile):
    binding = None
    if profile["state"] == "ready":
        binding = {
            "installation_id": profile["installation_id"],
            "lifecycle_id": profile["lifecycle_id"],
            "endpoint": {"kind": "unix", "path": profile["socket"]},
            "credential_reference": {
                "kind": "dotenv", "path": profile["credential"], "key": "BACKBONE_API_KEY"},
            "tmux": {"socket_path": profile["tmux"]},
        }
    return {"schema_version": 1, **item(profile), "binding": binding, "git_identity": "none"}


# ------------------------------------------------------------------ CLI ----

def emit(value, code=0):
    sys.stdout.write(json.dumps(value) + "\n")
    sys.stdout.flush()
    return code


def error(code, message, exit_code=1, **extra):
    return emit({"schema_version": 1, **extra, "error": {"code": code, "message": message}}, exit_code)


def option(args, name):
    for index, arg in enumerate(args):
        if arg.startswith(name + "="):
            return arg.split("=", 1)[1]
        if arg == name and index + 1 < len(args):
            return args[index + 1]
    return None


def after_dashes(args):
    return args[args.index("--") + 1] if "--" in args and args.index("--") + 1 < len(args) else (args[-1] if args else None)


def cli(argv):
    if argv[:2] == ["profiles", "list"]:
        with locked(write=False) as holder:
            state = holder["state"]
        if not state or not state["profiles"]:
            return error("registry_absent", "No profiles exist yet.")
        return emit({"schema_version": 1, "items": [item(p) for p in state["profiles"]]})

    if argv[:2] == ["profiles", "show"]:
        profile_id = after_dashes(argv[2:])
        with locked(write=False) as holder:
            profile = find(holder["state"] or {"profiles": []}, profile_id)
        if profile is None:
            return error("profile_not_found", "Profile was not found.")
        return emit(record(profile))

    if argv[:2] == ["profiles", "operation"]:
        request_id = after_dashes(argv[2:])
        with locked(write=False) as holder:
            receipt = (holder["state"] or {"operations": {}})["operations"].get(request_id)
        if receipt is None:
            return error("operation_not_found", "Profile operation was not found.", request_id=request_id)
        return emit(receipt, 0 if receipt["status"] != "failed" else 1)

    if argv[:2] == ["profiles", "apply"]:
        request = json.loads(Path(option(argv, "--file")).read_text())
        log("lifecycle.jsonl", {"request": request})
        return apply(request)

    if argv[:1] == ["runtimes"]:
        return runtimes(argv)

    if argv[:1] == ["setup"]:
        request = json.loads(Path(option(argv, "--file")).read_text())
        log("setup.jsonl", {"request": request})
        return setup(request)

    if argv[:1] in (["--version"], ["version"]):
        print("agent-backbone 0.5.0 (stand-in)")
        return 0
    sys.stderr.write(f"stand-in backbone: unsupported command {argv!r}\n")
    return 2


def receipt_for(request, profile, status="succeeded", failure=None):
    ids = identity(profile) if profile else {"profile_id": None, "installation_id": None, "lifecycle_id": None}
    return {"schema_version": 1, "request_id": request["request_id"], "action": request["action"],
            **ids, "status": status, "error": failure}


def apply(request):
    request_id = request["request_id"]
    action = request["action"]
    with locked() as holder:
        state = holder["state"]
        previous = state["operations"].get(request_id)
        if previous is not None:
            return emit(previous, 0 if previous["status"] != "failed" else 1)
        if action == "create":
            if request.get("baseline_version") != BASELINE or not request.get("display_name", "").strip():
                receipt = receipt_for(request, None, "failed", {"code": "invalid_request", "message": "The request is not valid."})
                state["operations"][request_id] = receipt
                return emit(receipt, 1)
            profile_id = "p" + secrets.token_hex(4)
            profile = new_profile(profile_id, request["display_name"].strip(), "isolated", state["new_accounts"])
            state["profiles"].append(profile)
            state["operations"][request_id] = receipt_for(request, profile, "pending")
        else:
            profile = find(state, request.get("profile_id"))
            expected = {"profile_id": request.get("profile_id"),
                        "installation_id": request.get("expected_installation_id"),
                        "lifecycle_id": request.get("expected_lifecycle_id")}
            if profile is None or identity(profile) != expected or profile["state"] != "ready":
                receipt = receipt_for(request, profile, "failed",
                                      {"code": "binding_mismatch", "message": "Profile binding does not match."})
                if profile is None:
                    receipt.update(expected)
                state["operations"][request_id] = receipt
                return emit(receipt, 1)
            state["operations"][request_id] = receipt_for(request, profile, "pending")
            profile["running"] = action == "start"
    if action == "create":
        time.sleep(CREATE_DELAY)  # provisioning
        with locked() as holder:
            state = holder["state"]
            profile = find(state, profile_id)
            profile["state"] = "ready"
            receipt = receipt_for(request, profile)
            state["operations"][request_id] = receipt
        return emit(receipt)
    if action == "start":
        # The service answers before start reports success, as the real one does.
        deadline = time.monotonic() + 10
        socket = Path(profile["socket"])
        while not socket.exists() and time.monotonic() < deadline:
            time.sleep(0.05)
        time.sleep(START_DELAY)
    with locked() as holder:
        state = holder["state"]
        profile = find(state, request["profile_id"])
        ok = action != "start" or Path(profile["socket"]).exists()
        receipt = receipt_for(request, profile, "succeeded" if ok else "failed",
                              None if ok else {"code": "start_failed", "message": "The service did not answer."})
        state["operations"][request_id] = receipt
    return emit(receipt, 0 if ok else 1)


def catalog_items(profile):
    items = []
    for runtime_id, name, installed in RUNTIMES:
        supported = runtime_id != "shell"
        compatible = installed and supported
        items.append({
            "id": runtime_id,
            "display_name": name,
            "available": installed,
            "setup_supported": supported,
            "reports_state": "hooks + terminal" if supported else "terminal",
            "models": [],
            "efforts": [],
            "unattended": {"supported": False, "args": [], "sandboxed": False},
            "authentication": profile["accounts"].get(runtime_id, "not_applicable" if runtime_id == "shell" else "unknown") if installed else "unavailable",
            "setup_compatibility": {
                "compatible": compatible,
                "reason": "compatible" if compatible else ("setup_unsupported" if not supported else "runtime_missing"),
                "message": "The selected runtime meets setup prerequisites; authentication is unverified." if compatible
                           else ("First-agent setup is unavailable for this runtime." if not supported else "This runtime is not installed."),
                "path": str(ROOT / ".local/bin" / runtime_id) if installed else None,
                "version": VERSIONS.get(runtime_id) if installed else None,
                "version_error": None,
            },
        })
    return items


def runtimes(argv):
    profile_id = option(argv, "--profile")
    expected = {"profile_id": profile_id,
                "installation_id": option(argv, "--expected-installation-id"),
                "lifecycle_id": option(argv, "--expected-lifecycle-id")}
    log("cli-runtimes.jsonl", {"argv": argv})
    with locked(write=False) as holder:
        profile = find(holder["state"], profile_id)
    if profile is None or identity(profile) != expected:
        return error("binding_mismatch", "Profile binding does not match.")
    return emit({"schema_version": 2, **identity(profile), "items": catalog_items(profile)})


def setup(request):
    action = request["action"]
    ids = {"profile_id": request["profile_id"],
           "installation_id": request["expected_installation_id"],
           "lifecycle_id": request["expected_lifecycle_id"]}
    agent = request["agent"]
    base = {"schema_version": 2, "request_id": request["request_id"], **ids, "action": action}
    with locked() as holder:
        state = holder["state"]
        profile = find(state, request["profile_id"])
        done = state["setups"].get(f"{action}:{request['request_id']}")
    if done is not None:
        return emit(done, 0 if done["status"] == "completed" else 3)
    if profile is None or identity(profile) != ids:
        return emit({**base, "seq": 1, "type": "result", "status": "failed", "code": "binding_mismatch",
                     "message": "Profile binding does not match.", "retry": "none",
                     "completed_steps": [], "data": {}}, 1)
    if action == "cancel_first_agent":
        result = {**base, "seq": 1, "type": "result", "status": "completed", "code": "request_cancelled",
                  "message": "This setup request is cancelled; a new request may be submitted.",
                  "retry": "new_request", "completed_steps": [],
                  "data": {"cancelled_request_id": request["request_id"], "can_start_new_request": True}}
        with locked() as holder:
            holder["state"]["setups"][f"{action}:{request['request_id']}"] = result
        return emit(result)
    seq = 0
    completed = []
    steps = ["preflight", "hooks", "agent", "launch", "ready"]
    for step in steps:
        if step == "hooks" and not request.get("hooks"):
            seq += 1
            emit({**base, "seq": seq, "type": "step", "step": step, "status": "skipped"})
            continue
        seq += 1
        emit({**base, "seq": seq, "type": "step", "step": step, "status": "running"})
        time.sleep(STEP_DELAY)
        if step == "agent":
            if agent.get("create_directory"):
                Path(agent["directory"]).mkdir(parents=True, exist_ok=True)
            with locked() as holder:
                p = find(holder["state"], request["profile_id"])
                p["agents"] = [a for a in p["agents"] if a["name"] != agent["name"]] + [{
                    "name": agent["name"], "state": "starting", "runtime": agent["runtime"],
                    "registration_id": secrets.token_hex(16), "cwd": agent["directory"],
                    "since": now()}]
        if step == "ready":
            with locked() as holder:
                p = find(holder["state"], request["profile_id"])
                for a in p["agents"]:
                    if a["name"] == agent["name"]:
                        a["state"] = "idle"
                        a["since"] = now()
        seq += 1
        completed.append(step)
        emit({**base, "seq": seq, "type": "step", "step": step, "status": "completed"})
    seq += 1
    result = {**base, "seq": seq, "type": "result", "status": "completed", "code": "agent_ready",
              "message": "Agent started; it may show its own first-run screens in its terminal. Any greeting receipt records submission only.",
              "retry": "none", "completed_steps": completed,
              "data": {"agent_name": agent["name"], "launch_id": "launch-" + secrets.token_hex(6),
                       "ready": "ready", "greeting_delivery_id": None, "evidence": []}}
    with locked() as holder:
        holder["state"]["setups"][f"{action}:{request['request_id']}"] = result
    return emit(result)


# --------------------------------------------------------------- server ----

def respond(handler, status, value):
    body = json.dumps(value).encode()
    handler.send_response(status)
    handler.send_header("Content-Type", "application/json")
    handler.send_header("Content-Length", str(len(body)))
    handler.send_header("Connection", "close")
    handler.end_headers()
    handler.wfile.write(body)
    handler.close_connection = True
    return status


def make_handler(profile_id):
    class Handler(http.server.BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"

        def log_message(self, *_):
            pass

        def handle_request(self, method):
            length = int(self.headers.get("Content-Length") or 0)
            body = self.rfile.read(length) if length else b""
            status = self.route(method, body)
            log("http.jsonl", {"profile": profile_id, "method": method, "path": self.path,
                               "status": status,
                               "bound": "X-Backbone-Profile-ID" in self.headers})

        def do_GET(self):
            self.handle_request("GET")

        def do_POST(self):
            self.handle_request("POST")

        def do_DELETE(self):
            self.handle_request("DELETE")

        def route(self, method, body):
            with locked(write=False) as holder:
                profile = find(holder["state"], profile_id)
            if profile is None or not profile["running"]:
                return respond(self, 503, {"detail": {"code": "unavailable", "message": "Not running."}})
            if self.headers.get("Authorization") != f"Bearer {profile['key']}":
                return respond(self, 401, {"detail": "Unauthorized"})
            sent = {"profile_id": self.headers.get("X-Backbone-Profile-ID"),
                    "installation_id": self.headers.get("X-Backbone-Installation-ID"),
                    "lifecycle_id": self.headers.get("X-Backbone-Lifecycle-ID")}
            ids = identity(profile)
            if any(sent.values()):
                if sent != ids:
                    return respond(self, 409, {"detail": {"code": "binding_mismatch", "message": "Profile binding does not match."}})
            elif profile["kind"] != "legacy_default":
                return respond(self, 409, {"detail": {"code": "binding_required", "message": "Profile binding is required."}})
            url = urllib.parse.urlsplit(self.path)
            query = dict(urllib.parse.parse_qsl(url.query))
            path = url.path
            if method == "GET" and path == "/api/profile":
                return respond(self, 200, {"schema_version": 1, **ids, "baseline_version": BASELINE,
                                           "tmux": {"socket_path": profile["tmux"]}})
            if method == "GET" and path == "/api/agents":
                items = [{"name": a["name"], "state": a["state"], "online": True, "configured": True,
                          "runtime": a["runtime"], "registration_id": a["registration_id"],
                          "reason": None, "current_issue": None, "state_since": a["since"],
                          "cwd": a["cwd"]} for a in profile["agents"]]
                return respond(self, 200, {"items": items, "has_more": False, "next_cursor": None})
            if method == "GET" and path == "/api/profile/accounts/status":
                runtime = query.get("runtime", "")
                if runtime not in profile["accounts"] and runtime not in dict((r[0], r) for r in RUNTIMES):
                    return respond(self, 422, {"detail": {"code": "runtime_unavailable", "message": "This runtime cannot be signed in here."}})
                return respond(self, 200, {"schema_version": 1, "runtime": runtime,
                                           "authentication": profile["accounts"].get(runtime, "not_authenticated")})
            if method == "POST" and path == "/api/profile/accounts/sign-in":
                runtime = json.loads(body or b"{}").get("runtime")
                return respond(self, 200, {"schema_version": 1, **ids, "runtime": runtime,
                                           "session": f"sign-in-{runtime}", "created": True})
            if method == "POST" and path == "/api/profile/shells":
                shell_id = json.loads(body or b"{}").get("shell_id")
                return respond(self, 200, {"schema_version": 1, **ids, "shell_id": shell_id,
                                           "session": f"shell-{shell_id}", "created": True})
            if method == "GET":
                return respond(self, 200, {"items": [], "has_more": False, "next_cursor": None})
            return respond(self, 404, {"detail": "Not Found"})

    return Handler


class Server(socketserver.ThreadingUnixStreamServer):
    daemon_threads = True


def serve():
    """Bind each running profile's socket, and keep doing so as profiles start."""
    servers = {}
    (FAKE / "serve.pid").write_text(str(os.getpid()))
    while True:
        try:
            with locked(write=False) as holder:
                profiles = list(holder["state"]["profiles"])
        except (json.JSONDecodeError, FileNotFoundError, TypeError):
            time.sleep(0.05)
            continue
        for profile in profiles:
            key = profile["profile_id"]
            if profile["running"] and key not in servers:
                socket = Path(profile["socket"])
                private_dir(socket.parent)
                if socket.exists():
                    socket.unlink()
                server = Server(str(socket), make_handler(key))
                socket.chmod(0o600)
                threading.Thread(target=server.serve_forever, daemon=True).start()
                servers[key] = server
                log("serve.jsonl", {"bound": key, "socket": str(socket)})
            elif not profile["running"] and key in servers:
                servers.pop(key).shutdown()
                Path(profile["socket"]).unlink(missing_ok=True)
                log("serve.jsonl", {"stopped": key})
        time.sleep(0.05)


def main():
    argv = sys.argv[1:]
    if argv[:1] == ["serve"]:
        serve()
        return 0
    if argv[:1] == ["init"]:
        accounts = json.loads(argv[1]) if len(argv) > 1 else {}
        print(json.dumps(record(init(accounts))))
        return 0
    started = time.monotonic()
    code = 99
    try:
        code = cli(argv)
        return code
    finally:
        log("cli.jsonl", {"argv": argv, "exit": code, "seconds": round(time.monotonic() - started, 3)})


if __name__ == "__main__":
    sys.exit(main())
