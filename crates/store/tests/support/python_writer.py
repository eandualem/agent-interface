"""JSON-lines worker for an existing synthetic v2 library; no viewer or tmux."""
import json
import sqlite3
import sys
from contextlib import closing
from pathlib import Path

from tmux_workspaces.model import LayoutConflict
from tmux_workspaces.persistence import Store

root = Path(sys.argv[1]).resolve(strict=True)
# Refuse a missing/uninitialized fixture before Store can initialize or migrate it.
with closing(sqlite3.connect((root / "layouts.db").as_uri() + "?mode=ro", uri=True)) as db:
    if db.execute("SELECT value FROM terminal_layout WHERE id=1").fetchone() is None:
        raise ValueError("fixture needs terminal_layout row 1")

with closing(Store(root)) as store:
    model = store.load()  # Retain this Store, its base, and this Model across commands.

    def emit(status):
        print(json.dumps({"status": status, "state": model.state, "base": store.base,
                          "in_transaction": store.db.in_transaction,
                          "foreign_keys": store.db.execute("PRAGMA foreign_keys").fetchone()[0],
                          "sqlite_version": sqlite3.sqlite_version}), flush=True)

    emit("loaded")
    for line in sys.stdin:
        command = json.loads(line)
        op = command["op"]
        if op == "quit":
            break
        if op == "state":
            model.state = command["state"]
            emit("edited")
            continue
        if op in ("workspace_name", "tab_name"):
            items = model.state["workspaces"]
            if op == "tab_name":
                items = [tab for space in items for tab in space["tabs"]]
            next(item for item in items if item["id"] == command["id"])["name"] = command["name"]
            emit("edited")
        elif op == "save":
            try:
                store.save(model)
            except LayoutConflict:
                emit("conflict")  # Already reset to latest + retained local navigation.
            else:
                emit("saved")
        else:
            raise ValueError(f"unknown fixture operation: {op}")
