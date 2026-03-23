#!/usr/bin/env python3
"""Helper for just tg-* commands. Reads JSON from stdin or makes HTTP calls."""
import json
import sys
import urllib.error
import urllib.parse
import urllib.request


def _load():
    return json.load(sys.stdin)


def _post(url: str, params: dict) -> dict:
    import os
    data = urllib.parse.urlencode(params).encode()
    req = urllib.request.Request(url, data=data)
    # Bypass system proxy for tg-bot-api (localhost/127.0.0.1)
    os.environ["no_proxy"] = "127.0.0.1,localhost"
    os.environ["NO_PROXY"] = "127.0.0.1,localhost"
    try:
        with urllib.request.urlopen(req, timeout=10) as resp:
            return json.loads(resp.read())
    except urllib.error.HTTPError as e:
        return json.loads(e.read())
    except Exception as e:
        return {"ok": False, "description": str(e)}


cmd = sys.argv[1] if len(sys.argv) > 1 else ""

if cmd == "token":
    r = _load()
    print(r["result"]["token"])

elif cmd == "state":
    r = _load()
    res = r.get("result") or {}
    print(res.get("authorization_state") or r.get("description", "error"))

elif cmd == "status":
    r = _load()
    if r.get("ok"):
        u = r["result"]
        uname = u.get("username", "")
        fname = u.get("first_name", "")
        lname = u.get("last_name", "")
        uid = u.get("id", "")
        tag = f"@{uname}" if uname else "(no username)"
        print(f"  user: {tag} ({fname} {lname}, id={uid})")
    else:
        print(f"  error: {r.get('description', 'unknown')}")

elif cmd == "watch-token":
    # Usage: tg-login.py watch-token <user_db_path> <phone_digits> <snapshot_file>
    # snapshot_file="snap"  — snapshot mode: print all current tokens and exit
    # snapshot_file=<path>  — watch mode: wait for token not in snapshot, print it
    import time
    import re
    db_path, phone_digits, snapshot_file = sys.argv[2], sys.argv[3], sys.argv[4]

    def read_tokens(db_path, phone_digits):
        try:
            with open(db_path, "rb") as f:
                data = f.read()
            pattern = rb'(' + phone_digits.encode() + rb':[A-Za-z0-9_\-]{40,})'
            return set(m.group(1).decode() for m in re.finditer(pattern, data))
        except Exception:
            return set()

    if snapshot_file == "snap":
        # Snapshot mode: print existing tokens
        for t in read_tokens(db_path, phone_digits):
            print(t)
        sys.exit(0)

    # Watch mode: load snapshot, wait for new token
    try:
        with open(snapshot_file) as f:
            known = set(f.read().splitlines())
    except FileNotFoundError:
        known = set()

    deadline = time.monotonic() + 60
    while time.monotonic() < deadline:
        current = read_tokens(db_path, phone_digits)
        new = current - known
        if new:
            print(next(iter(new)))
            sys.exit(0)
        time.sleep(0.2)

    sys.exit(1)

elif cmd == "proxy-add":
    # Usage: tg-login.py proxy-add <tg_url> <token> <host> <port>
    # Retries until addProxy succeeds (TDLib may not be ready immediately after userLogin).
    import time
    tg_url, token, host, port = sys.argv[2], sys.argv[3], sys.argv[4], sys.argv[5]
    base = f"{tg_url}/user{token}"

    deadline = time.monotonic() + 30
    while True:
        existing = _post(f"{base}/getProxies", {})
        for p in (existing.get("result") or []):
            if p.get("server") == host and str(p.get("port")) == port and p.get("is_enabled"):
                print("[proxy] Already configured")
                sys.exit(0)

        r = _post(f"{base}/addProxy", {"server": host, "port": port, "type": "socks5"})
        if r.get("ok"):
            proxy_id = r["result"]["id"]
            r2 = _post(f"{base}/enableProxy", {"proxy_id": proxy_id})
            if r2.get("ok"):
                print(f"[proxy] SOCKS5 {host}:{port} configured (id={proxy_id})")
                sys.exit(0)

        if time.monotonic() > deadline:
            print(f"[proxy] WARNING: gave up after 30s: {r.get('description')}", file=sys.stderr)
            sys.exit(0)

        print(f"[proxy] Waiting for session to accept proxy config...", file=sys.stderr)
        time.sleep(2)
