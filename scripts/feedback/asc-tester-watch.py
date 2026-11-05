#!/usr/bin/env python3
"""Add every accepted App Store Connect invitee to the "Arbos internal" TestFlight group.

Runs every 3 minutes from asc-tester-watch.timer on the relay host (installed as /usr/local/bin/asc-tester-watch,
env from /etc/arbos/feedback.env: ASC_KEY_ID, ASC_ISSUER_ID, ASC_KEY_P8_B64).

Who is a candidate — no hand-kept list any more (2026-10-22: Sami accepted his invite and was never added
because the list in the unit's ExecStart named three people from Oct 15):
  * every pending user invitation (GET /userInvitations) — remembered in the state file, because an accepted
    invitation disappears from that list;
  * every ASC user with the CUSTOMER_SUPPORT role (the role our tester invites carry);
  * emails on the command line and in /etc/arbos/asc-testers.txt (one per line), for anything odd.
For each candidate whose ASC user exists: make sure a betaTester for the app exists and is in the group
(POST /betaTesters with the group; 409 = exists → add to the group explicitly). Admin/account-holder users
are left alone. State: /var/lib/arbos-feedback/internal-testers.json (per email: done, tester id/state, when).
"""
import base64
import datetime
import json
import os
import sys
import time
import urllib.error
import urllib.parse
import urllib.request

from cryptography.hazmat.primitives import hashes, serialization
from cryptography.hazmat.primitives.asymmetric import ec
from cryptography.hazmat.primitives.asymmetric.utils import decode_dss_signature

GROUP = os.environ.get("ASC_BETA_GROUP", "7e3176d2-b3cc-43e5-b757-3cec84930d0b")
APP = os.environ.get("ASC_APP_ID", "6812503407")
STATE = "/var/lib/arbos-feedback/internal-testers.json"
EXTRA_LIST = "/etc/arbos/asc-testers.txt"
TESTER_ROLES = {"CUSTOMER_SUPPORT"}


def now_iso() -> str:
    return datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def token() -> str:
    def b64(b: bytes) -> str:
        return base64.urlsafe_b64encode(b).rstrip(b"=").decode()

    key = serialization.load_pem_private_key(base64.b64decode(os.environ["ASC_KEY_P8_B64"]), None)
    now = int(time.time())
    h = b64(json.dumps({"alg": "ES256", "kid": os.environ["ASC_KEY_ID"], "typ": "JWT"}).encode())
    p = b64(json.dumps({"iss": os.environ["ASC_ISSUER_ID"], "iat": now, "exp": now + 600, "aud": "appstoreconnect-v1"}).encode())
    r, s = decode_dss_signature(key.sign(f"{h}.{p}".encode(), ec.ECDSA(hashes.SHA256())))
    return f"{h}.{p}.{b64(r.to_bytes(32, 'big') + s.to_bytes(32, 'big'))}"


class AscError(RuntimeError):
    """A read from App Store Connect that did not answer 200."""


def api(method: str, path: str, body: dict | None = None) -> tuple[int, dict]:
    req = urllib.request.Request(
        "https://api.appstoreconnect.apple.com/v1" + path,
        method=method,
        headers={"Authorization": "Bearer " + token(), "Content-Type": "application/json"},
        data=json.dumps(body).encode() if body else None,
    )
    try:
        with urllib.request.urlopen(req, timeout=30) as r:
            return r.status, json.loads(r.read() or b"{}")
    except urllib.error.HTTPError as e:
        return e.code, json.loads(e.read() or b"{}")


def read_ok(path: str) -> dict:
    """A GET whose status is not 200 answers with an error document, and `doc["data"] or []` turns that
    into an empty list — which reads exactly like an account with no invitations, no users and no testers.
    An expired key would therefore produce a silent, clean, exit-0 run every 3 minutes while nobody was
    being added: the 2026-10-22 miss (Sami accepted and was never added) by a different route. Only a 200
    is an answer; anything else stops the run so the timer goes red and says why."""
    code, doc = api("GET", path)
    if code != 200:
        raise AscError(f"GET {path} -> HTTP {code}: {json.dumps(doc)[:400]}")
    return doc


def candidates(argv_emails: list[str], state: dict) -> dict[str, str]:
    """email -> where it came from."""
    out: dict[str, str] = {}
    for e in argv_emails:
        out[e] = "argv"
    if os.path.exists(EXTRA_LIST):
        for line in open(EXTRA_LIST):
            e = line.split("#", 1)[0].strip().lower()
            if e:
                out.setdefault(e, "asc-testers.txt")
    for e in state:
        out.setdefault(e, "state")
    inv = read_ok("/userInvitations?limit=200")
    for u in inv.get("data") or []:
        e = (u["attributes"].get("email") or "").lower()
        if e:
            out.setdefault(e, "invitation")
    users = read_ok("/users?limit=200")
    for u in users.get("data") or []:
        a = u["attributes"]
        e = (a.get("username") or "").lower()
        if e and TESTER_ROLES & set(a.get("roles") or []):
            out.setdefault(e, "user")
    return out


def tester_for(email: str) -> dict | None:
    chk = read_ok("/betaTesters?" + urllib.parse.urlencode({"filter[email]": email, "filter[apps]": APP}))
    return (chk.get("data") or [None])[0]


def in_group(tester_id: str) -> bool:
    rel = read_ok(f"/betaTesters/{tester_id}/relationships/betaGroups")
    return any(g.get("id") == GROUP for g in rel.get("data") or [])


def main() -> None:
    argv_emails = [e.strip().lower() for e in (sys.argv[1] if len(sys.argv) > 1 else "").split(",") if e.strip()]
    state = json.load(open(STATE)) if os.path.exists(STATE) else {}
    users = read_ok("/users?limit=200")
    by_email = {(u["attributes"].get("username") or "").lower(): u for u in users.get("data") or []}
    for email, source in candidates(argv_emails, state).items():
        rec = state.get(email) or {}
        if rec.get("done"):
            continue
        user = by_email.get(email)
        if not user:
            state[email] = {**rec, "done": False, "source": rec.get("source") or source, "checked": now_iso(),
                            "note": "invitation not accepted yet"}
            continue
        a = user["attributes"]
        first = a.get("firstName") or "Tester"
        last = a.get("lastName") or "Tester"
        code, resp = api("POST", "/betaTesters", {"data": {"type": "betaTesters", "attributes": {"email": email, "firstName": first, "lastName": last},
                                                   "relationships": {"betaGroups": {"data": [{"type": "betaGroups", "id": GROUP}]}}}})
        tester = tester_for(email)
        added_to_group = None
        if tester and code == 409 and not in_group(tester["id"]):
            # The tester record existed (another group, or added by hand without the group): add the group.
            gcode, _ = api("POST", f"/betaGroups/{GROUP}/relationships/betaTesters", {"data": [{"type": "betaTesters", "id": tester["id"]}]})
            added_to_group = gcode
        done = tester is not None and (code == 201 or (code == 409 and (added_to_group in (None, 204) or in_group(tester["id"]))))
        state[email] = {"done": done, "source": rec.get("source") or source, "post_status": code, "post_body": resp if code >= 400 and code != 409 else None,
                        "group_post_status": added_to_group, "user": a, "tester_state": tester["attributes"].get("state") if tester else None,
                        "tester_id": tester["id"] if tester else None, "at": now_iso()}
        print(f"{email}: {'added' if done else 'NOT added'} (POST {code}{', group ' + str(added_to_group) if added_to_group else ''}, source {source})")
    os.makedirs(os.path.dirname(STATE), exist_ok=True)
    json.dump(state, open(STATE, "w"), indent=2)
    print(json.dumps({e: {"done": r.get("done"), "source": r.get("source"), "tester_state": r.get("tester_state")} for e, r in state.items()}))


if __name__ == "__main__":
    try:
        main()
    except AscError as e:
        print(f"asc-tester-watch: App Store Connect did not answer; nobody was checked this tick: {e}", file=sys.stderr)
        sys.exit(1)
