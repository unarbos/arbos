#!/usr/bin/env python3
"""Pick the Mac build the arbos.life download button hands out.

Order:
  1. the newest published, non-draft, non-prerelease GitHub release that
     carries an Arbos-*.dmg (drafts are invisible to the releases API, so a
     draft's asset can never be chosen);
  2. else the newest build in the dev channel's own feed
     (releases/download/dev/arbos-dev.json): a .dmg if the channel ever ships
     one, else the macOS zip.

The feed is a plain file download, so step 2 needs no API quota; step 1 uses
the API and is skipped on a 403 (set GITHUB_TOKEN to lift the anonymous limit).

Prints one JSON object. --json writes it where the site can serve it;
--redirect-caddy writes `redir /download/mac <url> 302` for the server;
--redirect-pages appends the Cloudflare Pages equivalent to a _redirects file.
"""

import argparse
import json
import os
import re
import sys
import urllib.error
import urllib.request

REPO = "unarbos/arbos"
API = f"https://api.github.com/repos/{REPO}/releases?per_page=30"
DEV_FEED = f"https://github.com/{REPO}/releases/download/dev/arbos-dev.json"
DMG = re.compile(r"^Arbos-.*\.dmg$", re.I)
ZIP = re.compile(r"^Arbos-.*-macos-arm64\.zip$", re.I)


def get_json(url):
    headers = {"Accept": "application/vnd.github+json", "User-Agent": "arbos.life"}
    token = os.environ.get("GITHUB_TOKEN")
    if token and "api.github.com" in url:
        headers["Authorization"] = f"Bearer {token}"
    with urllib.request.urlopen(urllib.request.Request(url, headers=headers), timeout=30) as resp:
        return json.load(resp)


def stable_dmg():
    try:
        releases = get_json(API)
    except (urllib.error.URLError, OSError) as e:
        print(f"releases API unavailable ({e}); dev feed only", file=sys.stderr)
        return None
    for rel in releases:
        if rel.get("draft") or rel.get("prerelease"):
            continue
        for asset in rel.get("assets", []):
            if DMG.match(asset["name"]):
                return {
                    "url": asset["browser_download_url"],
                    "name": asset["name"],
                    "size": asset["size"],
                    "tag": rel["tag_name"],
                    "channel": "stable",
                    "published": asset.get("updated_at") or rel.get("published_at"),
                }
    return None


def dev_build():
    feed = get_json(DEV_FEED)
    releases = sorted(feed.get("releases", []), key=lambda r: r.get("build", 0), reverse=True)
    for rel in releases:
        macs = [d for d in rel.get("downloads", []) if d.get("platform") == "macos" and not d.get("component")]
        macs.sort(key=lambda d: 0 if d.get("format") == "dmg" else 1)
        if macs:
            d = macs[0]
            return {
                "url": d["url"],
                "name": d["url"].rsplit("/", 1)[-1],
                "size": d.get("size"),
                "tag": feed.get("channel", "dev"),
                "channel": "dev",
                "version": rel.get("version"),
                "build": rel.get("build"),
                "commit": rel.get("commit"),
                "minimum_system_version": rel.get("minimum_system_version"),
                "published": rel.get("published"),
            }
    return None


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--json")
    ap.add_argument("--redirect-caddy")
    ap.add_argument("--redirect-pages")
    args = ap.parse_args()

    info = stable_dmg() or dev_build()
    if not info:
        print("no published Mac build found", file=sys.stderr)
        return 1
    info["format"] = "dmg" if info["name"].lower().endswith(".dmg") else "zip"
    if "version" not in info:
        m = re.match(r"^Arbos-(\d+\.\d+\.\d+)", info["name"])
        info["version"] = m.group(1) if m else None
    out = json.dumps(info, indent=2) + "\n"
    print(out, end="")
    if args.json:
        with open(args.json, "w") as f:
            f.write(out)
    if args.redirect_caddy:
        with open(args.redirect_caddy, "w") as f:
            f.write(f"# written by mac-download.py: {info['name']}\nredir /download/mac {info['url']} 302\n")
    if args.redirect_pages:
        with open(args.redirect_pages, "a") as f:
            f.write(f"/download/mac {info['url']} 302\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
