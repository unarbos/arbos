#!/usr/bin/env bash
# agentd-canary-smoke.sh <sha> <image-version> <snapshot>
#
# Called by arbos-deploy-worker (roll_image) after the snapshot is registered and BEFORE the
# relay default switches or any real box moves: pin one test user's box to the new snapshot,
# wait for it to come up on it, run the busy-reply gate (busy-reply-smoke.py) against it, clear
# the pin. Exit 0 = gate held, 1 = gate FAILED (the worker stops the roll), 2 = the smoke could
# not run (reported, not blocking). Events smoke_ok / smoke_failed go to the deploy events file.
#
# Environment from the worker: RELAY_URL, RELAY_ENV, STATE, EVENTS, SMOKE_USER_ID (empty = skip),
# SMOKE_GATE_S (default 5), SMOKE_WAIT_S (default 900).
set -uo pipefail
sha="${1:?sha}" version="${2:?version}" snapshot="${3:?snapshot}"
RELAY_URL="${RELAY_URL:-http://127.0.0.1:7700}"
RELAY_ENV="${RELAY_ENV:-/etc/arbos/relay.env}"
STATE="${STATE:-/var/lib/arbos-deploy}"
EVENTS="${EVENTS:-/var/lib/arbos-feedback/deploy-events.jsonl}"
SMOKE_USER_ID="${SMOKE_USER_ID-test_01M50KGEEN5RRVNRRPFBMF2N8R}"
SMOKE_GATE_S="${SMOKE_GATE_S:-5}"
SMOKE_WAIT_S="${SMOKE_WAIT_S:-900}"
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
mkdir -p "$STATE/runs"

log() { echo "$(date -u +%Y-%m-%dT%H:%M:%SZ) agentd: canary: $*"; }
event() {
  # event <name> <detail>
  python3 - "$EVENTS" "$sha" "$1" "$2" <<'EOF2' 2>/dev/null || true
import json, sys, time
path, sha, name, detail = sys.argv[1:5]
line = json.dumps({"at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()), "event": name,
                   "deploy": "agentd", "sha": sha, "detail": detail})
with open(path, "a") as fh:
    fh.write(line + "\n")
EOF2
}
admin="$(sed -n 's/^ARBOS_RELAY_ADMIN_TOKEN=//p' "$RELAY_ENV" | head -1 | tr -d '"')"
[[ -z "$SMOKE_USER_ID" ]] && { log "no SMOKE_USER_ID; smoke skipped"; exit 2; }
[[ -z "$admin" ]] && { log "no admin token in $RELAY_ENV; smoke skipped"; exit 2; }
pin() {
  curl -fsS --max-time 20 -X PUT -H "Authorization: Bearer $admin" -H 'content-type: application/json' \
    -d "$1" "$RELAY_URL/admin/users/$SMOKE_USER_ID/box" >/dev/null
}
unpin() { pin '{"snapshot":null}' || true; }

pin "{\"snapshot\":\"$snapshot\"}" || { log "could not pin $SMOKE_USER_ID to $snapshot"; exit 2; }
log "upgrading $SMOKE_USER_ID → $snapshot: $(ARBOS_RELAY_ADMIN_TOKEN="$admin" ARBOSCTL_CONFIG=/tmp/none.json \
  /usr/local/bin/arbosctl --relay "$RELAY_URL" admin upgrade-box "$SMOKE_USER_ID" --force 2>&1 | tr -d '\n' | cut -c1-200)"
t0=$(date +%s); up=0
while (( $(date +%s) - t0 < SMOKE_WAIT_S )); do
  sleep 15
  if curl -fsS --max-time 20 -H "Authorization: Bearer $admin" "$RELAY_URL/admin/users" | python3 -c '
import sys, json
want_user, want_ver = sys.argv[1], sys.argv[2]
for u in json.load(sys.stdin):
    if u["id"] == want_user:
        m = u.get("machine") or {}
        ok = m.get("state") == "awake" and not (m.get("upgrade") or {}).get("in_progress") \
            and (m.get("agentd") or {}).get("image") == want_ver
        sys.exit(0 if ok else 1)
sys.exit(1)' "$SMOKE_USER_ID" "$version"; then up=1; break; fi
done
if (( up == 0 )); then
  log "$SMOKE_USER_ID did not come up on image $version within $SMOKE_WAIT_S s"
  unpin
  exit 2
fi
# Seed check INSIDE the running box (#1708): agentd reports the `.source-sha` of the pi-ext seed
# it copies into its projects (`machine.agentd.seed_sha`). A persisted /etc that kept an older
# image's seed means the box runs old extension code whatever the image says; hold the lane.
# An agentd too old to report the field (null) is logged, not blocking.
in_box_seed="$(curl -fsS --max-time 20 -H "Authorization: Bearer $admin" "$RELAY_URL/admin/users" | python3 -c '
import sys, json
for u in json.load(sys.stdin):
    if u["id"] == sys.argv[1]:
        print(((u.get("machine") or {}).get("agentd") or {}).get("seed_sha") or "")
' "$SMOKE_USER_ID" 2>/dev/null | tr -d '[:space:]')"
if [[ -z "$in_box_seed" ]]; then
  log "in-box seed check skipped: $SMOKE_USER_ID reports no seed_sha (agentd older than #1708)"
elif [[ "$in_box_seed" != "$sha"* && "$sha" != "$in_box_seed"* ]]; then
  event smoke_failed "pi-ext seed INSIDE the canary box is $in_box_seed, image $version was built from $sha — the box's /etc did not follow the image (#1708)"
  log "in-box seed check FAILED: box seed $in_box_seed != $sha"
  unpin
  exit 1
else
  log "in-box seed check ok — $SMOKE_USER_ID runs pi-ext seed $in_box_seed"
fi
sleep 20
report="$(ARBOS_RELAY_ADMIN_TOKEN="$admin" python3 "$here/busy-reply-smoke.py" \
  --relay "$RELAY_URL" --user "$SMOKE_USER_ID" --gate-seconds "$SMOKE_GATE_S" \
  --log "$STATE/runs/agentd-$version-smoke.log" 2>&1 | tail -1)"
echo "$report" > "$STATE/runs/agentd-$version-smoke.json"
unpin
rc="$(python3 -c 'import json, sys
try:
    r = json.loads(sys.argv[1])
except Exception:
    sys.exit(2)
sys.exit(0 if r.get("ok") is True else (1 if r.get("ok") is False else 2))' "$report" && echo 0 || echo $?)"
short="$(echo "$report" | cut -c1-300)"
case "$rc" in
  0) event smoke_ok "busy-reply gate held on $SMOKE_USER_ID (image $version): $short"; log "smoke OK: $report" ;;
  1) event smoke_failed "busy-reply gate FAILED on $SMOKE_USER_ID (image $version): $short"; log "smoke FAILED: $report" ;;
  *) log "smoke could not run (not blocking): $report" ;;
esac
exit "$rc"
