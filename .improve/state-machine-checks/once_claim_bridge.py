"""Bounded generated test seam for exact production schema and claim UPDATE SQL.

This executes SQLite, not the Rust receipt/handler boundary (covered natively).
No project module imports, effects, disk databases, credentials or network.
"""
import hashlib
import json
from pathlib import Path
import re
import sqlite3
import sys

SOURCE = Path(__file__).resolve().parents[2] / "crates/arbos-relay/src/db_secrets.rs"
text = SOURCE.read_text()
schemas = re.findall(r'pub const SECRETS_SCHEMA: &str = r#"(.*?)"#;', text, re.S)
section = text.split("pub(crate) fn claim_allowed_once_batch(", 1)[1].split("// ---- settings", 1)[0]
queries = re.findall(r'"(UPDATE secret_requests SET status = \'expired\'.*?)",\s*params!', section, re.S)
if len(schemas) != 1 or len(queries) != 1:
    raise SystemExit("Production SQL extraction changed")
SCHEMA, CLAIM = schemas[0], queries[0]
FIELDS = ("id", "user_id", "project_id", "chat_id", "target_kind", "target_id", "target_ref",
          "use_kind", "host", "asked_by", "decided_by", "decided_at", "expires_at")
NULLABLE = {"target_id", "host", "asked_by", "decided_by", "decided_at"}
STATUSES = {"allowed_once", "expired", "pending", "denied"}
MAX_LINE = 65536


def integer(value):
    return type(value) is int and 0 <= value <= 10**12


def snapshot(value, stored=False):
    if not isinstance(value, dict) or set(value) != set(FIELDS) | ({"status"} if stored else set()):
        raise ValueError("Invalid trace")
    for key in FIELDS:
        item = value[key]
        if item is None and key in NULLABLE:
            continue
        if key in {"decided_at", "expires_at"}:
            if not integer(item):
                raise ValueError("Invalid trace")
        elif not isinstance(item, str) or not 1 <= len(item) <= 64:
            raise ValueError("Invalid trace")
    if stored and value["status"] not in STATUSES:
        raise ValueError("Invalid trace")


def run(request):
    if not isinstance(request, dict) or set(request) != {"id", "grants", "events"} or not integer(request["id"]):
        raise ValueError("Invalid trace")
    grants, events = request["grants"], request["events"]
    if not isinstance(grants, list) or not 1 <= len(grants) <= 3 or not isinstance(events, list) or len(events) > 32:
        raise ValueError("Invalid trace")
    for grant in grants:
        snapshot(grant, stored=True)
    ids = [grant["id"] for grant in grants]
    if len(set(ids)) != len(ids):
        raise ValueError("Invalid trace")
    for event in events:
        if not isinstance(event, dict) or set(event) != {"claims", "now", "fail_id"} or not integer(event["now"]):
            raise ValueError("Invalid trace")
        if event["fail_id"] is not None and event["fail_id"] not in ids:
            raise ValueError("Invalid trace")
        claims = event["claims"]
        if not isinstance(claims, list) or len(claims) > 3:
            raise ValueError("Invalid trace")
        for claim in claims:
            snapshot(claim)
        if len({claim["id"] for claim in claims}) != len(claims):
            raise ValueError("Invalid trace")
    with sqlite3.connect(":memory:") as connection:
        connection.executescript(SCHEMA)
        keys = (*FIELDS, "status", "created_at")
        connection.executemany("INSERT INTO secret_requests (" + ",".join(keys) + ") VALUES (" + ",".join("?" for _ in keys) + ")",
                               [[grant[key] for key in (*FIELDS, "status")] + [0] for grant in grants])
        connection.commit()
        steps = []
        for event in events:
            connection.execute("DROP TRIGGER IF EXISTS fixture_failure")
            if event["fail_id"] is not None:
                # IDs are bound into a temporary guard table, never interpolated SQL.
                connection.execute("CREATE TEMP TABLE IF NOT EXISTS fail_id (id TEXT)")
                connection.execute("DELETE FROM fail_id")
                connection.execute("INSERT INTO fail_id VALUES (?)", (event["fail_id"],))
                connection.execute("CREATE TEMP TRIGGER fixture_failure BEFORE UPDATE ON main.secret_requests WHEN NEW.id IN (SELECT id FROM fail_id) BEGIN SELECT RAISE(ABORT, 'synthetic failure'); END")
            connection.commit()
            outcome = "Claimed"
            try:
                connection.execute("BEGIN")
                for claim in event["claims"]:
                    if connection.execute(CLAIM, [claim[key] for key in FIELDS] + [event["now"]]).rowcount != 1:
                        outcome = "Unavailable"
                        break
                if outcome == "Claimed":
                    connection.commit()
                else:
                    connection.rollback()
            except sqlite3.Error:
                connection.rollback()
                outcome = "SqlError"
            rows = connection.execute("SELECT id, status FROM secret_requests ORDER BY id").fetchall()
            steps.append({"outcome": outcome, "rows": [list(row) for row in rows]})
    return {"id": request["id"], "steps": steps, "sqlite": sqlite3.sqlite_version,
            "python": ".".join(map(str, sys.version_info[:3])), "sql_sha256": hashlib.sha256(CLAIM.encode()).hexdigest()}


def main():
    while True:
        raw = sys.stdin.buffer.readline(MAX_LINE + 1)
        if not raw:
            break
        if len(raw) > MAX_LINE:
            while raw and not raw.endswith(b"\n"):
                raw = sys.stdin.buffer.readline(MAX_LINE + 1)
            print(json.dumps({"id": None, "error": "Invalid trace"}), flush=True)
            continue
        identity = None
        try:
            request = json.loads(raw)
            if isinstance(request, dict) and integer(request.get("id")):
                identity = request["id"]
            reply = run(request)
        except (ValueError, TypeError, KeyError, sqlite3.Error):
            reply = {"id": identity, "error": "Invalid trace"}
        print(json.dumps(reply, separators=(",", ":")), flush=True)


if __name__ == "__main__":
    main()
