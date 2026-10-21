"""Run separate Python/Rust writers on disposable libraries at the reference pin.

Usage: python3 crates/store/tests/mixed_writers.py /path/to/python/checkout
Requires Python 3.11+, Cargo and rustc. Does not access tmux or a real library.
"""
import copy
import json
import os
from pathlib import Path
import select
import sqlite3
import subprocess
import sys
import tempfile

PIN = "51de6757cd0aeda3c19fe68fa4ec5b93ab206804"
HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]


def shared(state):
    result = json.loads(json.dumps(state))
    for key in ("selected", "focus"):
        result.pop(key, None)
    for space in result["workspaces"]:
        space.pop("selected", None)
        for tab in space["tabs"]:
            tab.pop("focus", None)
    return result


class Worker:
    def __init__(self, command, env):
        self.process = subprocess.Popen(
            command, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
            stderr=subprocess.PIPE, text=True, env=env,
        )

    def receive(self):
        ready, _, _ = select.select([self.process.stdout], [], [], 15)
        if not ready:
            raise AssertionError("writer response timed out")
        line = self.process.stdout.readline()
        if not line:
            raise AssertionError(self.process.stderr.read())
        return json.loads(line)

    def send(self, command):
        self.process.stdin.write(json.dumps(command) + "\n")
        self.process.stdin.flush()
        return self.receive()

    def close(self):
        if self.process.poll() is None:
            self.process.stdin.write('{"op":"quit"}\n')
            self.process.stdin.flush()
            try:
                self.process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.wait()
        assert self.process.returncode == 0, self.process.stderr.read()


def scenarios():
    record = {
        "version": 2, "selected": "000000000001", "focus": False,
        "extension": {"nullable": None, "number": 123456789012345678901234567890},
        "workspaces": [{
            "id": "000000000001", "name": "Workspace", "selected": "000000000002",
            "tabs": [{"id": "000000000002", "name": "Tab", "focus": "000000000003",
                      "tree": {"id": "000000000003", "agent": "editor", "source_socket": "/tmp/offline-example.sock"}},
                     {"id": "000000000004", "name": "Second tab", "focus": "000000000005",
                      "tree": {"id": "000000000005", "agent": None, "empty": True}}],
        }, {"id": "000000000006", "name": "Other workspace", "selected": "", "tabs": []},
           {"id": "000000000009", "name": "Third workspace", "selected": "", "tabs": []}],
    }

    def split(tab, identity, direction):
        tab["tree"] = {"id": f"{identity:012x}", "direction": direction, "ratio": 0.5,
                       "first": tab["tree"], "second": {"id": f"{identity + 1:012x}", "agent": None, "empty": True}}

    def move(state, destination):
        tab = state["workspaces"][0]["tabs"].pop(0)
        state["workspaces"][0]["selected"] = "000000000004"
        state["workspaces"][destination]["tabs"].append(tab)
        state["workspaces"][destination]["selected"] = tab["id"]

    cases = []
    for name in ("independent-edits", "conflict-and-retry", "independent-splits", "conflicting-splits",
                 "stale-delete", "move-and-rename", "conflicting-moves", "incompatible-reorder",
                 "independent-additions", "navigation-fallback"):
        a, b, expected = (copy.deepcopy(record) for _ in range(3))
        conflict = name in ("conflict-and-retry", "conflicting-splits", "stale-delete", "conflicting-moves", "incompatible-reorder")
        if name in ("independent-edits", "conflict-and-retry"):
            a["workspaces"][0]["name"] = "First edit"
            if conflict:
                b["workspaces"][0]["name"] = "Second edit"
            else:
                b["workspaces"][0]["tabs"][0]["name"] = "Second edit"
                expected["workspaces"][0]["name"] = "First edit"
                expected["workspaces"][0]["tabs"][0]["name"] = "Second edit"
        elif name in ("independent-splits", "conflicting-splits"):
            split(a["workspaces"][0]["tabs"][0], 10, "right")
            split(b["workspaces"][0]["tabs"][0 if conflict else 1], 12, "below")
            expected["workspaces"][0]["tabs"] = [a["workspaces"][0]["tabs"][0], b["workspaces"][0]["tabs"][1]]
        elif name == "stale-delete":
            a["workspaces"][0]["tabs"][0]["name"] = "Peer rename"
            b["workspaces"][0]["tabs"].pop(0)
            b["workspaces"][0]["selected"] = "000000000004"
        elif name in ("move-and-rename", "conflicting-moves"):
            move(a, 1)
            if conflict:
                move(b, 2)
            else:
                b["workspaces"][2]["name"] = "Peer workspace name"
                expected = copy.deepcopy(a)
                expected["workspaces"][2]["name"] = "Peer workspace name"
        elif name == "incompatible-reorder":
            a["workspaces"] = [a["workspaces"][1], a["workspaces"][0], a["workspaces"][2]]
            b["workspaces"] = [b["workspaces"][0], b["workspaces"][2], b["workspaces"][1]]
        elif name == "independent-additions":
            tab = {"id": "00000000000a", "name": "Added tab", "focus": "00000000000b",
                   "tree": {"id": "00000000000b", "agent": None, "empty": True}}
            workspace = {"id": "00000000000c", "name": "Added workspace", "tabs": [], "selected": ""}
            a["workspaces"][0]["tabs"].append(tab)
            b["workspaces"].append(workspace)
            expected["workspaces"][0]["tabs"].append(tab)
            expected["workspaces"].append(workspace)
        elif name == "navigation-fallback":
            a["workspaces"].pop(0)
            a["selected"] = "000000000006"
            b["workspaces"][2]["name"] = "Peer workspace name"
            expected = copy.deepcopy(a)
            expected["workspaces"][1]["name"] = "Peer workspace name"
        cases.append((name, record, a, b, a if conflict else expected, conflict))
    return cases


def run_case(binary, env, first, scenario):
    name, record, a, b, expected, conflict = scenario
    with tempfile.TemporaryDirectory(prefix="library-writer-") as temporary:
        directory = Path(temporary)
        with sqlite3.connect(directory / "layouts.db", timeout=1) as db:
            db.execute("CREATE TABLE terminal_layout(id INTEGER PRIMARY KEY, value TEXT)")
            db.execute("INSERT INTO terminal_layout VALUES(1, ?)", (json.dumps(record),))
            db.execute("CREATE TABLE layout(id INTEGER PRIMARY KEY, value TEXT)")
            db.execute("INSERT INTO layout VALUES(1, ' legacy bytes ')")
            db.execute("CREATE TABLE extension_row(parent INTEGER REFERENCES terminal_layout(id) ON DELETE CASCADE, value TEXT)")
            db.execute("INSERT INTO extension_row VALUES(1, ' retained extension ')")
        commands = {
            "rust": [str(binary), temporary],
            "python": [sys.executable, str(HERE / "support/python_writer.py"), temporary],
        }
        workers = {name: Worker(command, env) for name, command in commands.items()}
        try:
            loaded = {name: worker.receive() for name, worker in workers.items()}
            assert all(value["status"] == "loaded" for value in loaded.values())
            assert loaded["python"]["foreign_keys"] == 0
            assert shared(loaded["rust"]["base"]) == shared(loaded["python"]["base"])
            second = "python" if first == "rust" else "rust"
            assert workers[first].send({"op": "state", "state": a})["status"] == "edited"
            assert workers[second].send({"op": "state", "state": b})["status"] == "edited"
            committed = workers[first].send({"op": "save"})
            assert committed["status"] == "saved"
            with sqlite3.connect(directory / "layouts.db") as db:
                before = db.execute("SELECT value FROM terminal_layout WHERE id=1").fetchone()[0]
            result = workers[second].send({"op": "save"})
            assert result["status"] == ("conflict" if conflict else "saved")
            if conflict:
                with sqlite3.connect(directory / "layouts.db") as db:
                    assert db.execute("SELECT value FROM terminal_layout WHERE id=1").fetchone()[0] == before
                assert shared(result["state"]) == shared(committed["state"])
                assert shared(result["base"]) == shared(committed["state"])
                workers[second].send({"op": "workspace_name", "id": "000000000001", "name": "Explicit retry"})
                result = workers[second].send({"op": "save"})
                assert result["status"] == "saved"
                expected = copy.deepcopy(expected)
                next(space for space in expected["workspaces"] if space["id"] == "000000000001")["name"] = "Explicit retry"
            assert shared(result["state"]) == shared(expected), (name, shared(result["state"]), shared(expected))
            assert shared(result["base"]) == shared(expected), name
            if name == "navigation-fallback":
                assert result["state"]["selected"] == "000000000006"
            if name == "move-and-rename":
                assert result["state"]["workspaces"][0]["selected"] == "000000000004"
            with sqlite3.connect(directory / "layouts.db", timeout=1) as db:
                # Waiting workers must not retain even a read transaction.
                db.execute("BEGIN EXCLUSIVE")
                assert shared(json.loads(db.execute("SELECT value FROM terminal_layout WHERE id=1").fetchone()[0])) == shared(result["state"])
                assert db.execute("SELECT value FROM layout").fetchone()[0] == " legacy bytes "
                assert db.execute("SELECT value FROM extension_row").fetchone()[0] == " retained extension "
                assert db.execute("PRAGMA journal_mode").fetchone()[0] == "delete"
                db.rollback()
            for command in commands.values():
                reopened = Worker(command, env)
                try:
                    assert shared(reopened.receive()["state"]) == shared(result["state"])
                finally:
                    reopened.close()
        finally:
            for worker in workers.values():
                worker.close()
    return {"first": first, "case": name, "passed": True,
            "python_sqlite": loaded["python"]["sqlite_version"], "python_foreign_keys": loaded["python"]["foreign_keys"]}


def main():
    reference = Path(sys.argv[1]).resolve(strict=True)
    head = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=reference, text=True).strip()
    assert head == PIN, f"reference checkout must be at {PIN}"
    assert not subprocess.check_output(["git", "status", "--porcelain", "--untracked-files=no"], cwd=reference, text=True), "reference tracked files must be clean"
    env = os.environ.copy()
    env["PYTHONPATH"] = str(reference)
    env["PYTHONDONTWRITEBYTECODE"] = "1"
    artifacts = subprocess.check_output(
        ["cargo", "build", "-p", "tmux-workspaces-store", "--message-format=json"], cwd=ROOT, text=True,
    )
    libraries = {}
    for line in artifacts.splitlines():
        event = json.loads(line)
        if event["reason"] == "compiler-artifact":
            for filename in event["filenames"]:
                if filename.endswith(".rlib"):
                    libraries[event["target"]["name"]] = filename
    with tempfile.TemporaryDirectory(prefix="writer-binary-") as temporary:
        binary = Path(temporary) / "rust-writer"
        command = ["rustc", "--edition=2024", str(HERE / "support/writer.rs"), "-o", str(binary),
                   "-L", "dependency=" + str(Path(libraries["tmux_workspaces_store"]).parent / "deps")]
        for name in ("tmux_workspaces_store", "tmux_workspaces_core", "serde_json"):
            command += ["--extern", name + "=" + libraries[name]]
        subprocess.run(command, check=True)
        results = [run_case(binary, env, first, scenario)
                   for first in ("python", "rust") for scenario in scenarios()]
    print(json.dumps({"python_revision": head, "results": results}, indent=2))


if __name__ == "__main__":
    main()
