import hashlib
import importlib.util
import json
import os
import re
import sqlite3
import stat
import sys
from datetime import datetime, timezone
from pathlib import Path

spec = importlib.util.spec_from_file_location("query_loader", "/query_loader.py")
loader = importlib.util.module_from_spec(spec)
spec.loader.exec_module(loader)
params = json.load(sys.stdin)
directory = Path("/snapshots")
name = params.pop("snapshot")
params.setdefault("chat_id", None)
params.setdefault("input_message_id", None)
if name == "latest":
    names = sorted(
        p.name for p in directory.iterdir()
        if re.fullmatch(r"restored-[a-zA-Z0-9._-]+\.db", p.name)
    )
    if not names:
        raise ValueError("No inactive restore. Run just backup run and just backup restore latest.")
    name = names[-1]
if not re.fullmatch(r"restored-[a-zA-Z0-9._-]+\.db", name):
    raise ValueError("Invalid inactive restore basename")
snapshot = directory / name
fd = os.open(snapshot, os.O_RDONLY | os.O_NOFOLLOW)
metadata = os.fstat(fd)
if not stat.S_ISREG(metadata.st_mode) or Path(str(snapshot) + "-wal").exists():
    raise ValueError("Snapshot must be an inactive regular restore without WAL")
# Bind SQLite to the opened file, not a pathname that could be replaced.
db = sqlite3.connect(f"file:/proc/self/fd/{fd}?mode=ro&immutable=1", uri=True)
db.row_factory = sqlite3.Row
rows = []
for result in db.execute(loader.load_sql(Path("/queries"), "snapshot"), params):
    row = dict(result)
    incoming = row.pop("_input") or ""
    body = row.pop("_body") or ""
    words = incoming.split()
    tail = words[2:]
    alias = tail.pop(0) if tail and tail[0] in {"nobullshit", "noshit", "tale"} else None
    focus = " ".join(tail)
    row.update(
        alias=alias,
        focus_characters=len(focus),
        focus_requests_brevity="двух словах" in focus or "two words" in focus,
        text_bytes=len(body.encode()),
        text_characters=len(body),
        text_sha256=hashlib.sha256(body.encode()).hexdigest(),
    )
    rows.append(row)
schema = dict(db.execute(loader.load_sql(Path("/queries"), "schema")).fetchone())
print(json.dumps({"path": "data/native-restore/" + name, "modified_at": datetime.fromtimestamp(metadata.st_mtime, timezone.utc).isoformat(), "schema": schema, "deliveries": rows}))
db.close()
os.close(fd)
