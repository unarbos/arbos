#!/usr/bin/env python3
"""TestFlight beta feedback from App Store Connect — screenshots with their
text, and crashes — for the iPhone loop. Each new submission becomes a
folder <out>/<date>-<n>/ with the image files, feedback.json and
feedback.md, and one summary line is printed per new item; "nothing new"
otherwise. Never prints the key, the token or a tester's address.

Credentials come from the environment (the same names as the CI secrets):

    ASC_KEY_ID      or IOS_ASC_KEY_ID      the key's id
    ASC_ISSUER_ID   or IOS_ASC_ISSUER_ID   the issuer uuid
    ASC_KEY_P8      or IOS_ASC_KEY_P8      the .p8 contents (PEM)
    ASC_APP_ID                             the app (default: Arbos iOS)
    FEEDBACK_OUT                           output folder (default ./feedback-out)

State is the output folder itself: a submission is "seen" when a
<date>-<n>/feedback.json with its id exists there, so the folder is the
only thing to keep (the loop copies it into the project store). A legacy
seen.json in the folder is honoured too.

    asc-feedback.py [--all]   # --all: re-list everything, ignore seen
"""
import datetime
import json
import os
import pathlib
import sys
import time
import urllib.error
import urllib.parse
import urllib.request

import jwt  # PyJWT + cryptography


def env(*names, default=None):
    for name in names:
        value = os.environ.get(name)
        if value:
            return value
    if default is not None:
        return default
    sys.exit(f"missing {' or '.join(names)} in the environment")


APP_ID = env("ASC_APP_ID", default="6812503407")
OUT = pathlib.Path(env("FEEDBACK_OUT", default="feedback-out")).expanduser()
OUT.mkdir(parents=True, exist_ok=True)


def token():
    key_id = env("ASC_KEY_ID", "IOS_ASC_KEY_ID")
    issuer = env("ASC_ISSUER_ID", "IOS_ASC_ISSUER_ID")
    key = env("ASC_KEY_P8", "IOS_ASC_KEY_P8").replace("\\n", "\n")
    now = int(time.time())
    return jwt.encode(
        {"iss": issuer, "iat": now, "exp": now + 900, "aud": "appstoreconnect-v1"},
        key, algorithm="ES256", headers={"kid": key_id},
    )


def get(path, params, tok):
    url = "https://api.appstoreconnect.apple.com" + path + "?" + urllib.parse.urlencode(params)
    req = urllib.request.Request(url, headers={"Authorization": f"Bearer {tok}"})
    try:
        with urllib.request.urlopen(req, timeout=30) as r:
            return r.status, json.load(r)
    except urllib.error.HTTPError as e:
        body = e.read().decode("utf-8", "replace")
        try:
            body = json.loads(body)
        except ValueError:
            pass
        return e.code, body


def seen_ids():
    ids = set()
    for record in OUT.glob("*/feedback.json"):
        try:
            ids.add(json.loads(record.read_text())["id"])
        except (ValueError, KeyError, OSError):
            continue
    legacy = OUT / "seen.json"
    if legacy.exists():
        try:
            ids.update(json.loads(legacy.read_text()))
        except ValueError:
            pass
    return ids


def main():
    seen = set() if "--all" in sys.argv else seen_ids()
    tok = token()
    new = []
    feeds = [
        ("screenshot", f"/v1/apps/{APP_ID}/betaFeedbackScreenshotSubmissions"),
        ("crash", f"/v1/apps/{APP_ID}/betaFeedbackCrashSubmissions"),
    ]
    for kind, path in feeds:
        status, body = get(path, {"include": "build,tester", "sort": "-createdDate", "limit": "50"}, tok)
        if status != 200:
            print(f"{kind}: HTTP {status}: {json.dumps(body)[:600]}")
            continue
        included = {(i["type"], i["id"]): i for i in body.get("included", [])}
        items = body.get("data", [])
        print(f"{kind}: {len(items)} submission(s) on the account")
        for item in items:
            if item["id"] in seen:
                continue
            a = item.get("attributes", {})
            rel = item.get("relationships", {})
            build = included.get(("builds", (rel.get("build", {}).get("data") or {}).get("id")))
            build_no = (build or {}).get("attributes", {}).get("version", "?")
            tester = included.get(("betaTesters", (rel.get("tester", {}).get("data") or {}).get("id")))
            created = a.get("createdDate", "")
            day = created[:10] or datetime.date.today().isoformat()
            n = 1
            while (OUT / f"{day}-{n}").exists():
                n += 1
            folder = OUT / f"{day}-{n}"
            folder.mkdir()
            files = []
            for i, shot in enumerate(a.get("screenshots", []) or [], 1):
                url = shot.get("url")
                if not url:
                    continue
                ext = ".png" if ".png" in url.lower() else ".jpg"
                dest = folder / f"screenshot-{i}{ext}"
                try:
                    urllib.request.urlretrieve(url, dest)
                    files.append(dest.name)
                except (urllib.error.URLError, OSError) as e:
                    print(f"  screenshot {i}: download failed: {e}")
            raw = {k: v for k, v in a.items() if k != "screenshots"}
            record = {
                "id": item["id"], "kind": kind, "created": created, "build": build_no,
                "comment": a.get("comment", ""), "tester": "[removed]" if tester else "unknown",
                "device": a.get("deviceModel", ""), "os": a.get("osVersion", ""), "locale": a.get("locale", ""),
                "appPlatform": a.get("appPlatform", ""), "battery": a.get("batteryPercentage"),
                "connection": a.get("connectionType", ""), "files": files, "raw": raw,
            }
            (folder / "feedback.json").write_text(json.dumps(record, indent=2, default=str))
            (folder / "feedback.md").write_text(
                f"# TestFlight feedback ({kind}) — {created}\n\n"
                f"- build: {build_no}\n- device: {record['device']} · iOS {record['os']}\n- tester: {record['tester']}\n\n"
                f"## What the tester wrote\n\n{record['comment'] or '(no text)'}\n\n## Files\n\n"
                + "\n".join(f"- {f}" for f in files) + "\n"
            )
            seen.add(item["id"])
            new.append((folder.name, kind, build_no, record["device"], record["os"], record["comment"], files))
    for folder, kind, build_no, device, osv, comment, files in new:
        print(f"NEW {folder} [{kind}] build {build_no} {device} iOS {osv} files={len(files)} :: {comment[:200]!r}")
    if not new:
        print("nothing new")


if __name__ == "__main__":
    main()
