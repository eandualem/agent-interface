#!/usr/bin/env python3
"""A stand-in for the Rust Agent Backbone service in the Linux first-run
regression: its `backbone` CLI and the sockets of the profiles it serves,
in the shapes the service prints and serves (its protocol fixtures, as this
repository copies them under crates/roster).

Three roles, one file:

  fake_backbone.py init             a machine whose service runs with no profile
  fake_backbone.py serve            the service: each served profile's api.sock
  fake_backbone.py <backbone args>  the CLI: profiles list/show/create,
                                    runtimes, setup (initialize, first_agent)

The machine's first profile uses the machine user's accounts, every later
one its own, and account status reads the login files the service reads
for each: none is made here, so a login is present only where one is put.

Everything lives under FAKE_BACKBONE_ROOT (the test's temporary HOME):
  <backbone dir>/profiles/<id>/   each profile's api.sock, as the service
                                  keeps them: $XDG_DATA_HOME/backbone
  fake/state.json                 the machine, its profiles and their agents
  fake/*.jsonl                    logs of CLI calls, HTTP requests and setup

Nothing here starts tmux, an agent CLI or a network connection.
"""
import fcntl
import http.server
import json
import os
import secrets
import shutil
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
VERSION = "0.1.0"
# FAKE_BACKBONE_SLOW=1 adds delays so each busy state stays on screen long
# enough to watch or record.
SLOW = os.environ.get("FAKE_BACKBONE_SLOW") == "1"
STEP_DELAY = 0.75 if SLOW else 0.0

# `backbone runtimes --json`, as the service's catalogue lists them; only
# `available` depends on this machine: whether the runtime's executable is
# on PATH.
EXECUTABLES = {"claude": "claude", "codex": "codex", "opencode": "opencode", "antigravity": "agy"}
RUNTIMES = [
    {"id": "claude", "display_name": "Claude Code", "setup_supported": True,
     "reports_state": "hooks + terminal", "models": ["opus", "sonnet", "haiku"],
     "efforts": ["low", "medium", "high", "xhigh", "max"],
     "unattended": {"supported": True, "args": ["--dangerously-skip-permissions"], "sandboxed": False},
     "authentication": "unknown"},
    {"id": "codex", "display_name": "Codex", "setup_supported": True,
     "reports_state": "hooks + terminal", "models": ["gpt-5.6-sol", "gpt-6-astra"],
     "efforts": ["low", "medium", "high", "xhigh", "max", "ultra"],
     "unattended": {"supported": True, "args": ["-a", "never", "-s", "workspace-write"], "sandboxed": True},
     "authentication": "unknown"},
    {"id": "opencode", "display_name": "OpenCode", "setup_supported": True,
     "reports_state": "hooks + terminal", "models": [],
     "efforts": ["none", "minimal", "low", "medium", "high", "xhigh", "max"],
     "unattended": {"supported": True, "args": ["--auto"], "sandboxed": False},
     "authentication": "unknown"},
    {"id": "antigravity", "display_name": "Antigravity CLI", "setup_supported": False,
     "reports_state": "terminal", "models": [], "efforts": ["low", "medium", "high", "xhigh", "max"],
     "unattended": {"supported": True, "args": ["--dangerously-skip-permissions"], "sandboxed": False},
     "authentication": "unknown"},
    {"id": "shell", "display_name": "Plain shell", "setup_supported": False,
     "reports_state": "terminal", "models": [], "efforts": [],
     "unattended": {"supported": False, "args": [], "sandboxed": None},
     "authentication": "not_applicable"},
]
# The tools setup starts a first agent in.
SETUP_RUNTIMES = ("claude", "codex", "antigravity")


# ---------------------------------------------------------------- state ----

def now():
    return time.time()


def rfc3339(seconds):
    return time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime(seconds))


def private_dir(path):
    path.mkdir(parents=True, exist_ok=True)
    path.chmod(0o700)
    return path


def backbone_dir():
    """Where the service keeps its profiles: BACKBONE_DIR, else the XDG data
    directory, else ~/.local/share; relative values are ignored."""
    for name, below in (("BACKBONE_DIR", None), ("XDG_DATA_HOME", "backbone")):
        value = os.environ.get(name)
        if value and Path(value).is_absolute():
            return Path(value) / below if below else Path(value)
    return ROOT / ".local/share/backbone"


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


def init():
    """A machine whose service runs and serves no profile yet."""
    private_dir(FAKE)
    with locked() as holder:
        holder["state"] = {
            "machine": {"id": secrets.token_hex(8), "name": "onboarding-smoke"},
            "instance_id": None,
            "started": None,
            "profiles": [],
            "creates": {},
            "setups": {},
        }


def find(state, profile_id):
    return next((p for p in state["profiles"] if p["profile_id"] == profile_id), None)


def new_profile(state, display_name):
    profile_id = secrets.token_hex(6)
    base = private_dir(private_dir(private_dir(backbone_dir()) / "profiles") / profile_id)
    # The machine user's accounts for the machine's first profile, its own
    # for each further one.
    accounts = "own" if state["profiles"] else "machine_user"
    profile = {"profile_id": profile_id, "display_name": display_name, "accounts": accounts,
               "dir": str(base), "socket": str(base / "api.sock"),
               "tmux_socket": str(base / "tmux.sock"), "agents": []}
    state["profiles"].append(profile)
    return profile


def discovery(state, profile):
    """A served profile as its discovery file says it."""
    return {"protocol_version": 1, "backbone_version": VERSION,
            "profile_id": profile["profile_id"], "display_name": profile["display_name"],
            "instance_id": state["instance_id"], "machine": dict(state["machine"]), "home": True,
            "socket": profile["socket"], "tmux_socket": profile["tmux_socket"],
            "credential_reference": {"type": "socket"}, "accounts": profile["accounts"]}


def entry(state, profile):
    return {"state": "serving", **discovery(state, profile)}


def wait_served(profile):
    """The service answers once it serves the profile it made; one it never
    serves stops the stand-in, so the check fails on it."""
    deadline = time.monotonic() + 10
    while not Path(profile["socket"]).exists():
        if time.monotonic() >= deadline:
            sys.stderr.write(f"stand-in backbone: profile {profile['profile_id']} was never served\n")
            sys.exit(99)
        time.sleep(0.05)


def stored_login(state, profile, runtime):
    """Whether a login is stored, as the service tells it on Linux from
    metadata alone: Claude Code's and Codex's login files, in the machine
    user's home or the profile's own accounts folder; Antigravity CLI's in
    the home for every profile, else possibly in the keychain."""
    home = Path(os.environ["HOME"])
    if runtime == "antigravity":
        token = home / ".gemini/antigravity-cli/antigravity-oauth-token"
        return ("present" if token.is_file() else "unavailable",
                "Antigravity CLI keeps its login in the machine user's home, so every "
                "profile on this machine shares it")
    own = Path(profile["dir"]) / "accounts"
    files = {"claude": (own / "claude" if profile["accounts"] == "own" else home / ".claude") / ".credentials.json",
             "codex": (own / "codex" if profile["accounts"] == "own" else home / ".codex") / "auth.json"}
    return ("present" if files[runtime].is_file() else "missing"), None


# ------------------------------------------------------------------ CLI ----

def emit(value):
    # `--json` prints the service's reply as it came, pretty-printed.
    sys.stdout.write(json.dumps(value, indent=2) + "\n")
    sys.stdout.flush()
    return 0


def line(value):
    sys.stdout.write(json.dumps(value, separators=(",", ":")) + "\n")
    sys.stdout.flush()


def failed(message, code=1):
    sys.stderr.write(f"backbone: {message}\n")
    return code


def served_state():
    """The machine's state once its service answers, else None."""
    with locked(write=False) as holder:
        state = holder["state"]
    return state if state and state["instance_id"] else None


UNREACHABLE = "nothing is listening at the machine's service.sock; is `backbone daemon` running?"


def option(args, name):
    for index, arg in enumerate(args):
        if arg.startswith(name + "="):
            return arg.split("=", 1)[1]
        if arg == name and index + 1 < len(args):
            return args[index + 1]
    return None


def after_dashes(args):
    return args[args.index("--") + 1] if "--" in args and args.index("--") + 1 < len(args) else None


def unique(pairs):
    """An object from `pairs`, refusing a key said twice, at any depth."""
    keys = [key for key, _ in pairs]
    if len(keys) != len(set(keys)):
        raise ValueError("a key is said twice")
    return dict(pairs)


def cli(argv):
    # Every command only as the app runs it, with --json; anything else is
    # a usage error.
    shapes = {
        ("profiles", "list"): lambda a: a == ["--json"],
        ("profiles", "show"): lambda a: len(a) == 3 and a[:2] == ["--json", "--"],
        ("profiles", "create"): lambda a: len(a) == 7 and a[0::2][:3] == ["--starter", "--request-id", "--json"]
                                           and a[5] == "--",
        ("runtimes", "--json"): lambda a: a == [],
    }
    shape = shapes.get(tuple(argv[:2]))
    if (argv[:1] == ["runtimes"] or argv[:1] == ["profiles"]) and not (shape and shape(argv[2:])):
        return failed(f"usage: run as the app runs it, with --json, not {argv!r}", 2)
    if argv[:2] == ["profiles", "list"]:
        state = served_state()
        if state is None:
            return failed(UNREACHABLE, 3)
        return emit({"profiles": [entry(state, p) for p in state["profiles"]]})

    if argv[:2] == ["profiles", "show"]:
        state = served_state()
        if state is None:
            return failed(UNREACHABLE, 3)
        profile_id = after_dashes(argv[2:])
        profile = find(state, profile_id)
        if profile is None:
            return failed(f"profile {profile_id} is not on this machine; "
                          "`backbone profiles list` shows the profiles here")
        return emit(entry(state, profile))

    if argv[:2] == ["profiles", "create"]:
        return create(argv[2:])

    if argv[:1] == ["runtimes"]:
        return runtimes()

    if argv[:1] == ["setup"]:
        # Only as the app runs it: a request file, answered in JSON lines.
        if len(argv) != 4 or argv[1] != "--file" or argv[3] != "--jsonl":
            sys.stderr.write(f"stand-in backbone: setup is run as setup --file F --jsonl, not {argv!r}\n")
            return 2
        try:
            request = json.loads(Path(argv[2]).read_text(), object_pairs_hook=unique)
        except ValueError as error:
            # Refused before it runs, as setup refuses a request it cannot read.
            log("setup.jsonl", {"refused": str(error)})
            line({"schema_version": 3, "request_id": "", "seq": 1, "type": "result",
                  "action": "validate", "status": "failed", "code": "invalid_request",
                  "message": f"the request is not one this version takes: {error}",
                  "data": {}, "completed_steps": [], "retry": "new_request"})
            return 2
        log("setup.jsonl", {"request": request})
        return setup(request)

    if argv[:1] in (["--version"], ["version"]):
        print(f"backbone {VERSION} (stand-in)")
        return 0
    sys.stderr.write(f"stand-in backbone: unsupported command {argv!r}\n")
    return 2


def create(args):
    name = after_dashes(args)
    request_id = option(args, "--request-id")
    if option(args, "--starter") != "core-v1" or not request_id or name is None:
        return failed("usage: backbone profiles create NAME", 2)
    if served_state() is None:
        return failed(UNREACHABLE, 3)
    with locked() as holder:
        state = holder["state"]
        made = state["creates"].get(request_id)
        if made is not None:
            # A repeat under the same request ID gets the first result.
            profile = find(state, made)
        elif not (1 <= len(name) <= 64) or name != name.strip():
            return failed("the profile's name is not one the service takes (invalid_request)")
        elif any(p["display_name"].lower() == name.lower() for p in state["profiles"]):
            return failed(f"a profile is named {name} already (conflict)")
        else:
            profile = new_profile(state, name)
            state["creates"][request_id] = profile["profile_id"]
        created = entry(state, profile)
    wait_served(profile)
    log("create.jsonl", {"argv": args, "entry": created})
    return emit(created)


def runtimes():
    # No service is asked: what is on this PATH is available.
    path = os.environ.get("PATH", "")
    items = [{**r, "available": r["id"] == "shell"
              or shutil.which(EXECUTABLES[r["id"]], path=path) is not None}
             for r in RUNTIMES]
    return emit({"schema_version": 1, "items": items})


# ---------------------------------------------------------------- setup ----

def exit_code(result):
    if result["code"] == "invalid_request":
        return 2
    return {"completed": 0, "failed": 1, "action_required": 3}.get(result["status"], 1)


def events(request, steps, result):
    """Setup's JSON lines: each step, then the result."""
    head = {"schema_version": 3, "request_id": request["request_id"]}
    lines = [{**head, "seq": seq, "type": "step", "step": step, "status": status}
             for seq, (step, status) in enumerate(steps, start=1)]
    lines.append({**head, "seq": len(lines) + 1, "type": "result", "action": request["action"], **result})
    return lines


def setup(request):
    key = f"{request['action']}:{request['request_id']}"
    with locked(write=False) as holder:
        done = holder["state"]["setups"].get(key)
    if done is None:
        if request["action"] == "initialize":
            done = initialize(request)
        elif request["action"] == "first_agent":
            done = first_agent(request)
        else:
            return 2
        with locked() as holder:
            holder["state"]["setups"][key] = done
    for value in done:
        time.sleep(STEP_DELAY if value["type"] == "step" else 0)
        line(value)
    return exit_code(done[-1])


def initialize(request):
    """A machine with no profile makes its first one and serves it."""
    with locked() as holder:
        state = holder["state"]
        if state["profiles"]:
            existing = [entry(state, p) for p in state["profiles"]]
            return events(request, [("profile", "running"), ("profile", "completed")], {
                "status": "completed", "code": "existing_preserved",
                "message": "this machine has a profile already, kept as it was",
                "data": {"backbone_dir": str(backbone_dir()), "installation": "existing_preserved",
                         "profile": discovery(state, state["profiles"][0]), "existing": existing,
                         "service_state": "running", "health": "healthy"},
                "completed_steps": ["profile"], "retry": "none"})
        profile = new_profile(state, request["profile_name"])
        served = discovery(state, profile)
    wait_served(profile)
    steps = ["preflight", "service", "profile", "starter"]
    return events(request, [(step, status) for step in steps for status in ("running", "completed")], {
        "status": "completed", "code": "initialized",
        "message": "this machine is ready: its service runs, and its first profile is served",
        "data": {"backbone_dir": str(backbone_dir()), "installation": "created", "profile": served,
                 "service_state": "running", "health": "healthy"},
        "completed_steps": steps, "retry": "none"})


def first_agent(request):
    """The profile runs the request, then setup says each step it completed."""
    agent = request["agent"]
    with locked() as holder:
        state = holder["state"]
        profile = find(state, request["profile_id"])
        if profile is None:
            return events(request, [], {
                "status": "action_required", "code": "service_unavailable",
                "message": f"profile {request['profile_id']} is not served on this machine",
                "data": {}, "completed_steps": [], "retry": "same_request"})
        if agent["create_directory"]:
            Path(agent["directory"]).mkdir(parents=True, exist_ok=True)
        # The agent as the service's roster lists one, every field of its row.
        profile["agents"] = [a for a in profile["agents"] if a["name"] != agent["name"]] + [{
            "name": agent["name"], "session": agent["name"], "registration_id": secrets.token_hex(16),
            "configured": True, "runtime": agent["runtime"], "model": agent["model"],
            "model_source": "runtime_default", "dir": agent["directory"], "repo": "", "tags": [],
            "description": "", "watches": [], "state": "idle", "reason": None, "current_issue": None,
            "current_repo": None, "online": True, "plan_file": None, "plan_title": None,
            "tmux_created": rfc3339(now()), "tmux_attached": False, "tmux_windows": 1,
            "last_activity": now(), "state_since": now(), "last_message": None, "detail": None,
            "state_source": "push", "evidence": [], "machine": dict(state["machine"]), "waiting": None,
            # The profile's first registration is its main agent.
            "background": {"running": 0, "queued": 0, "processes": 0},
            "role": "main" if not profile["agents"] else "swarm_member",
            "superior": None, "mode": "sandboxed", "grants": [], "ceiling": [], "takes_issues": True}]
    steps = ["preflight", "agent", "launch", "ready"] + (["greeting"] if request["start_greeting"] else [])
    return events(request, [(step, "completed") for step in steps], {
        "status": "completed", "code": "agent_ready",
        "message": "the agent's CLI is running; it may ask the person something first",
        "data": {"agent_name": agent["name"], "ready": "ready", "launch_id": secrets.token_hex(16),
                 "greeting_delivery_id": 1 if request["start_greeting"] else None, "evidence": "idle"},
        "completed_steps": steps, "retry": "none"})


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


def refused(handler, status, code, message):
    return respond(handler, status, {"detail": {"code": code, "message": message}})


def make_handler(profile_id):
    class Handler(http.server.BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"

        def log_message(self, *_):
            pass

        def handle_request(self, method):
            length = int(self.headers.get("Content-Length") or 0)
            if length:
                self.rfile.read(length)
            self.said = None
            status = self.route(method)
            log("http.jsonl", {"profile": profile_id, "method": method, "path": self.path,
                               "status": status, "bound": "X-Backbone-Profile-ID" in self.headers,
                               **({"said": self.said} if self.said else {})})

        def do_GET(self):
            self.handle_request("GET")

        def do_POST(self):
            self.handle_request("POST")

        def route(self, method):
            with locked(write=False) as holder:
                state = holder["state"]
            profile = find(state, profile_id)
            url = urllib.parse.urlsplit(self.path)
            query = dict(urllib.parse.parse_qsl(url.query))
            if method == "GET" and url.path == "/health":
                return respond(self, 200, {"healthy": True, "components": {
                    "home": {"healthy": True}, "service": {"healthy": True}},
                    "version": VERSION, "started": state["started"]})
            # The owner-only socket is the credential; every request names its profile.
            named = self.headers.get("X-Backbone-Profile-ID")
            if named is None:
                return refused(self, 409, "binding_required", "name the profile this request is for")
            if named != profile_id:
                return refused(self, 409, "binding_mismatch", "this socket serves another profile")
            if method == "GET" and url.path == "/api/profile":
                served = discovery(state, profile)
                return respond(self, 200, {k: served[k] for k in (
                    "protocol_version", "backbone_version", "profile_id", "display_name",
                    "instance_id", "machine", "home")})
            if method == "GET" and url.path == "/api/agents":
                return respond(self, 200, {"items": profile["agents"], "total": len(profile["agents"])})
            if method == "GET" and url.path == "/api/profile/accounts/status":
                runtime = query.get("runtime", "")
                if runtime not in SETUP_RUNTIMES:
                    return refused(self, 422, "invalid_request", "this runtime keeps no account here")
                storage, message = stored_login(state, profile, runtime)
                self.said = storage
                return respond(self, 200, {"schema_version": 1, "profile_id": profile_id,
                                           "instance_id": state["instance_id"], "runtime": runtime,
                                           "storage": storage, **({"message": message} if message else {})})
            # Every other route, the push stream among them, is not this
            # stand-in's; the app goes on without it.
            return respond(self, 404, {"detail": {"code": "not_found", "message": "not found"}})

    return Handler


class Server(socketserver.ThreadingUnixStreamServer):
    daemon_threads = True


def serve():
    """The service: a new run of it, serving each profile's socket, and each
    profile made while it runs."""
    # Sockets are owner-only from the moment they exist, as the app checks.
    os.umask(0o177)
    with locked() as holder:
        holder["state"]["instance_id"] = secrets.token_hex(16)
        holder["state"]["started"] = rfc3339(now())
    servers = {}
    (FAKE / "serve.pid").write_text(str(os.getpid()))
    while True:
        with locked(write=False) as holder:
            profiles = list(holder["state"]["profiles"])
        for profile in profiles:
            key = profile["profile_id"]
            if key not in servers:
                socket = Path(profile["socket"])
                if socket.exists():
                    socket.unlink()
                server = Server(str(socket), make_handler(key))
                socket.chmod(0o600)
                threading.Thread(target=server.serve_forever, daemon=True).start()
                servers[key] = server
                log("serve.jsonl", {"bound": key, "socket": str(socket)})
        time.sleep(0.05)


def main():
    argv = sys.argv[1:]
    if argv[:1] == ["serve"]:
        serve()
        return 0
    if argv == ["init"]:
        init()
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
