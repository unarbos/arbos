#!/usr/bin/env bash
# Keep the public Mac download and the in-app update mirror on this host in step with the newest
# STABLE GitHub release:
#
#   1. /etc/arbos/download.caddy — https://arbos.life/download/mac (and relay.arbos.life/download/mac)
#      302-redirect to the newest Mac DMG; /download/mac/version is the version as plain text (the
#      site shows it on the Download button).
#   2. $WWW_DIR/updates/mac/ — the release's appcast.xml and …-macos-universal.tar.gz, served by Caddy
#      at https://arbos.life/updates/mac/<file> for the in-app updater (which tries this host first and
#      the GitHub asset second; it verifies the Ed25519 signature over the bytes either way). The two
#      newest versions are kept so a Mac mid-download is not cut off by a publish.
#   3. /etc/arbos/updates.d/mac.caddy — inside Caddy's /updates/mac handler: a file that is not
#      mirrored redirects to the same file under the current release tag on GitHub.
#
# Which release wins: always the latest STABLE release — GitHub's /releases/latest (the newest
# release that is neither a prerelease nor a draft; our tags are v<semver>) when it carries a .dmg.
# Edge releases (edge-<semver>.<build>, prereleases) are never offered on the public download, even
# when newer; they are only the fallback while no stable release exists at all.
#
# Installed as /usr/local/bin/arbos-refresh-mac-download by scripts/deploy/relay-server.sh; run by
# arbos-refresh-mac-download.timer every 15 minutes and by arbos-refresh-mac-download.path as soon as
# the relay records a stable Mac release (POST /admin/release). Caddy is reloaded only when something
# changed. Env: WWW_DIR (/home/const/arbos-www), WWW_USER (const), MIRROR_KEEP (2), MIRROR=0 to skip 2+3.
set -euo pipefail
OUT=/etc/arbos/download.caddy
UPDATES_SNIPPET=/etc/arbos/updates.d/mac.caddy
WWW_USER="${WWW_USER:-const}"
WWW_DIR="${WWW_DIR:-/home/$WWW_USER/arbos-www}"
MIRROR_DIR="$WWW_DIR/updates/mac"
MIRROR_KEEP="${MIRROR_KEEP:-2}"
MIRROR="${MIRROR:-1}"
RELOAD=1
[[ "${1:-}" == "--no-reload" ]] && RELOAD=0
# `changed` = anything changed (logged); `config_changed` = a Caddy snippet changed, the only thing a
# reload is for. Mirrored FILES (appcast.xml, the tar.gz) are served from disk and need no reload —
# until 2026-10-29 every mirrored release reloaded Caddy, and each reload closed every proxied
# WebSocket (49–57 boxes plus real clients, 12 times between 23:30 and 04:03) and twice broke an
# agentd canary smoke mid-probe (0.1.180 at 00:12, 0.1.184 at 04:02).
changed=0
config_changed=0
# Lane collision: while an agentd canary smoke (`agentd.canary`) or a relay deploy (`relay.deploying`)
# is in flight, a needed reload is DEFERRED — `www.reload-pending` is written and the deploy worker's
# tick (or the next run of this script) applies it once the lanes are clear. Markers older than
# LANE_MARKER_MAX_S are stale. `www.reloading` marks the reload itself for the canary to wait on.
DEPLOY_STATE="${ARBOS_DEPLOY_STATE:-/var/lib/arbos-deploy}"
LANE_MARKER_MAX_S="${LANE_MARKER_MAX_S:-1800}"
marker_fresh() { local f="$1" at; [[ -f "$f" ]] || return 1; at="$(stat -c %Y "$f" 2>/dev/null || echo 0)"; (( $(date +%s) - at < LANE_MARKER_MAX_S )); }
lanes_busy() { marker_fresh "$DEPLOY_STATE/agentd.canary" || marker_fresh "$DEPLOY_STATE/relay.deploying"; }
reload_www() { # the one place Caddy is reloaded; marked so a canary smoke starting now waits for it
  systemctl is-active --quiet arbos-www.service || return 0
  mkdir -p "$DEPLOY_STATE"
  printf 'by=%s\nat=%s\n' "arbos-refresh-mac-download" "$(date -u +%FT%TZ)" > "$DEPLOY_STATE/www.reloading"
  systemctl reload arbos-www.service || true
  rm -f "$DEPLOY_STATE/www.reloading" "$DEPLOY_STATE/www.reload-pending"
  echo "refresh-mac-download: arbos-www reloaded"
}
# A reload deferred by an earlier run is applied first when the lanes are clear now.
if [[ $RELOAD -eq 1 && -f "$DEPLOY_STATE/www.reload-pending" ]] && ! lanes_busy; then
  echo "refresh-mac-download: applying the reload deferred at $(sed -n 's/^at=//p' "$DEPLOY_STATE/www.reload-pending" | head -1)"
  reload_www
fi

api=https://api.github.com/repos/unarbos/arbos
latest=$(curl -fsSL "$api/releases/latest" 2>/dev/null || true)
# older stables + edges, for the fallbacks
json=$(curl -fsSL "$api/releases?per_page=60")
pick=$(printf '%s' "$json" | LATEST_JSON="$latest" python3 -c '
import os, re, sys, json

latest_raw = os.environ.get("LATEST_JSON", "")
rest = sys.stdin.read()

def asset(r, suffix):
    for a in r.get("assets", []):
        if a["name"].endswith(suffix):
            return a
    return None

def dmg(r):
    a = asset(r, ".dmg")
    return a["browser_download_url"] if a else None

def ver(tag, prefix):
    m = re.fullmatch(prefix + r"(\d+(?:\.\d+)*)", tag)
    return tuple(int(x) for x in m.group(1).split(".")) if m else None

def stable(r):
    return (not r.get("prerelease") and not r.get("draft") and ver(r["tag_name"], "v") and dmg(r))

best = None
try:
    latest = json.loads(latest_raw) if latest_raw.strip() else {}
except ValueError:
    latest = {}
if latest and stable(latest):
    best = (ver(latest["tag_name"], "v"), latest)
else:
    rels = json.loads(rest)
    stables = sorted(((ver(r["tag_name"], "v"), r) for r in rels if stable(r)), key=lambda t: t[0], reverse=True)
    if stables:
        best = stables[0]
    else:
        edges = sorted(((ver(r["tag_name"], "edge-"), r) for r in rels
                        if not r.get("draft") and dmg(r) and ver(r["tag_name"], "edge-")), key=lambda t: t[0], reverse=True)
        if edges:
            best = edges[0]
if best:
    v, r = best
    tar = asset(r, "-macos-universal.tar.gz")
    cast = asset(r, "appcast.xml")
    print(dmg(r)); print(".".join(str(x) for x in v)); print(r["tag_name"])
    # mirror lines: url<TAB>name<TAB>size, empty when the release has no such asset
    for a in (cast, tar):
        print("\t".join((a["browser_download_url"], a["name"], str(a["size"]))) if a else "")
')
dmg=$(printf '%s\n' "$pick" | sed -n 1p)
version=$(printf '%s\n' "$pick" | sed -n 2p)
tag=$(printf '%s\n' "$pick" | sed -n 3p)
appcast_line=$(printf '%s\n' "$pick" | sed -n 4p)
tar_line=$(printf '%s\n' "$pick" | sed -n 5p)
[[ -n "$dmg" ]] || { echo "refresh-mac-download: no release with a DMG" >&2; exit 1; }

# ---- 1. /download/mac redirect -------------------------------------------------------------------
mkdir -p /etc/arbos
new=$(printf '# generated by arbos-refresh-mac-download on %s (%s)\nredir /download/mac %s 302\nredir /download/mac/ %s 302\nrespond /download/mac/version "%s" 200\n' \
  "$(date -u +%FT%TZ)" "$tag" "$dmg" "$dmg" "$version")
if [[ ! -f "$OUT" ]] || [[ "$(grep -v '^#' "$OUT")" != "$(printf '%s' "$new" | grep -v '^#')" ]]; then
  printf '%s\n' "$new" > "$OUT"
  chmod 0644 "$OUT"
  echo "download/mac → $dmg ($tag)"
  changed=1
  config_changed=1
fi

# ---- 2. mirror for the in-app updater -----------------------------------------------------------
# Only stable tags are mirrored (the app only looks here for v* assets); an edge-only fallback
# release leaves the mirror as it is.
if [[ "$MIRROR" == 1 && "$tag" == v* ]]; then
  install -d -m 0755 -o "$WWW_USER" -g "$WWW_USER" "$WWW_DIR/updates" "$MIRROR_DIR"
  # Mirror one asset: download to a temp file beside the target, check the size GitHub reports,
  # move into place. Skipped when the file is already there with the right size.
  mirror_asset() {
    local url="$1" name="$2" size="$3" target tmp got
    target="$MIRROR_DIR/$name"
    if [[ -f "$target" ]] && [[ "$(stat -c %s "$target")" == "$size" ]]; then
      return 0
    fi
    tmp=$(mktemp "$MIRROR_DIR/.$name.XXXXXX")
    if ! curl -fsSL --retry 3 --retry-delay 5 --connect-timeout 30 --max-time 900 -o "$tmp" "$url"; then
      echo "refresh-mac-download: could not fetch $url" >&2; rm -f "$tmp"; return 1
    fi
    got=$(stat -c %s "$tmp")
    if [[ "$got" != "$size" ]]; then
      echo "refresh-mac-download: $name is $got bytes, GitHub says $size; not mirrored" >&2; rm -f "$tmp"; return 1
    fi
    chmod 0644 "$tmp"; chown "$WWW_USER:$WWW_USER" "$tmp"
    mv -f "$tmp" "$target"
    echo "updates/mac/$name ← $url ($size bytes)"
    changed=1
  }
  # The payload first: the appcast names it, so the manifest never points at a file that is not
  # here yet (the Caddy fallback would still redirect to GitHub, but why make the app take that hop).
  mirrored_tar=1
  if [[ -n "$tar_line" ]]; then
    IFS=$'\t' read -r url name size <<<"$tar_line"
    mirror_asset "$url" "$name" "$size" || mirrored_tar=0
  fi
  if [[ -n "$appcast_line" ]]; then
    IFS=$'\t' read -r url name size <<<"$appcast_line"
    # The appcast is rewritten on every publish, so GitHub's size is the only freshness check;
    # a same-size rewrite is caught by the content comparison below.
    tmp=$(mktemp "$MIRROR_DIR/.appcast.XXXXXX")
    if curl -fsSL --retry 3 --retry-delay 5 --connect-timeout 30 --max-time 120 -o "$tmp" "$url" \
       && [[ "$(stat -c %s "$tmp")" == "$size" ]] && grep -q '<rss' "$tmp"; then
      if [[ ! -f "$MIRROR_DIR/appcast.xml" ]] || ! cmp -s "$tmp" "$MIRROR_DIR/appcast.xml"; then
        chmod 0644 "$tmp"; chown "$WWW_USER:$WWW_USER" "$tmp"
        mv -f "$tmp" "$MIRROR_DIR/appcast.xml"
        echo "updates/mac/appcast.xml ← $url ($tag)"
        changed=1
      else
        rm -f "$tmp"
      fi
    else
      echo "refresh-mac-download: could not mirror $url" >&2; rm -f "$tmp"
    fi
  fi
  # Keep the newest MIRROR_KEEP payloads (by file mtime; a new version is always the newest file).
  if [[ "$mirrored_tar" == 1 ]]; then
    ls -1t "$MIRROR_DIR"/*-macos-universal.tar.gz 2>/dev/null | tail -n +"$((MIRROR_KEEP + 1))" | while read -r old; do
      rm -f "$old"; echo "pruned $(basename "$old")"
    done
  fi
  rm -f "$MIRROR_DIR"/.*.XXXXXX "$MIRROR_DIR"/.appcast.* "$MIRROR_DIR"/.*.tar.gz.* 2>/dev/null || true

  # ---- 3. fallback redirect for files the mirror does not hold ------------------------------------
  mkdir -p /etc/arbos/updates.d
  snippet=$(printf '# generated by arbos-refresh-mac-download on %s (%s)\n@mac_update_missing not file\nredir @mac_update_missing https://github.com/unarbos/arbos/releases/download/%s{path} 302\n' \
    "$(date -u +%FT%TZ)" "$tag" "$tag")
  if [[ ! -f "$UPDATES_SNIPPET" ]] || [[ "$(grep -v '^#' "$UPDATES_SNIPPET")" != "$(printf '%s' "$snippet" | grep -v '^#')" ]]; then
    printf '%s\n' "$snippet" > "$UPDATES_SNIPPET"
    chmod 0644 "$UPDATES_SNIPPET"
    echo "updates/mac fallback → github.com/unarbos/arbos/releases/download/$tag/"
    changed=1
    config_changed=1
  fi
fi

if [[ $config_changed -eq 1 && $RELOAD -eq 1 ]]; then
  if lanes_busy; then
    mkdir -p "$DEPLOY_STATE"
    printf 'at=%s\nreason=%s\n' "$(date -u +%FT%TZ)" "caddy snippet changed for $tag while an agentd canary smoke or a relay deploy is in flight" > "$DEPLOY_STATE/www.reload-pending"
    echo "refresh-mac-download: arbos-www reload DEFERRED (agentd canary / relay deploy in flight); the deploy worker applies it"
  else
    reload_www
  fi
elif [[ $changed -eq 1 ]]; then
  echo "refresh-mac-download: files mirrored, no Caddy config change — no reload"
fi
