#!/usr/bin/env bash
# Mesh health, from the cloud VM, every half hour: units, hub, roster truth, stale binaries, double-served places.
# Prints exactly one verdict line last: MESH-OK or MESH-FAULT <reasons>. Records one JSON line on the store-watch
# branch (mesh/arboslife.jsonl) so the history outlives this machine. Home: store-watch branch root.
set -uo pipefail
REPO="${REPO:-/workspace}"; BRANCH="${WATCH_BRANCH:-store-watch}"; NOW="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
say() { printf '[mesh-health %s] %s\n' "$(date -u +%H:%M:%SZ)" "$*" >&2; }
faults=()

# ---- ArbosLife, one ssh session ---------------------------------------------
remote=$(ssh -o ConnectTimeout=20 -o BatchMode=yes arboslife 'bash -s' <<'EOF' 2>/dev/null
echo "units=$(systemctl --user list-units "arbos-*" --no-pager --no-legend | awk "{print \$1\":\"\$3}" | tr "\n" ",")"
echo "healthz=$(curl -s -m 5 http://127.0.0.1:7010/healthz | tr -d "\n")"
T=$(grep -E "^\s*token" ~/arbos-hub/config/arbos/hub.toml | head -1 | sed -E "s/.*\"([^\"]+)\".*/\1/")
curl -s -m 8 -H "Authorization: Bearer $T" http://127.0.0.1:7010/list > /tmp/.mh-list.json 2>/dev/null
python3 - <<'PY'
import json
try:
    ms=json.load(open("/tmp/.mh-list.json"))["machines"]
except Exception as e:
    print("roster=UNREADABLE"); raise SystemExit
for m in ms:
    gone=[b["project"] or b["role"] for b in m.get("builds",[]) if b.get("binary_gone")]
    print(f"roster={m['name']} projects={','.join(sorted(p['name'] for p in m['projects']))} worker={m.get('worker')} builds={len(m.get('builds',[]))} gone={','.join(gone) or '-'} machine_sha={m.get('git_sha') or '-'}")
PY
rm -f /tmp/.mh-list.json
# stale or doubled processes (same test as sweep.sh, compact)
for p in $(pgrep -u "$USER" -x arbos-kernel; pgrep -u "$USER" -x arbos-hub); do
  exe=$(readlink /proc/$p/exe 2>/dev/null) || continue
  case "$exe" in *"(deleted)") echo "GONE=$p:$(tr '\0' ' ' < /proc/$p/cmdline | awk '{print $2,$3}')";; esac
done
for p in $(pgrep -u "$USER" -x arbos-kernel); do set -- $(tr '\0' ' ' < /proc/$p/cmdline 2>/dev/null); [ "$2" = serve ] && echo "$3"; done | sort | uniq -d | sed 's/^/DOUBLE=/'
echo "END"
EOF
)
rc=$?
if [ $rc -ne 0 ] || ! grep -q '^END$' <<<"$remote"; then faults+=("arboslife unreachable over ssh or the check did not finish"); fi
units=$(grep '^units=' <<<"$remote" | cut -d= -f2-)
inactive=$(tr ',' '\n' <<<"$units" | grep -v ':active' | grep . | tr '\n' ' ')
[ -n "$inactive" ] && faults+=("unit not active: $inactive")
nunits=$(tr ',' '\n' <<<"$units" | grep -c ':active')
[ "$nunits" -lt 6 ] && faults+=("only $nunits of 6 units active")
healthz=$(grep '^healthz=' <<<"$remote" | cut -d= -f2-)
[ "$healthz" = ok ] || faults+=("hub healthz: '${healthz:-no answer}'")
roster=$(grep '^roster=' <<<"$remote")
grep -q 'roster=UNREADABLE' <<<"$roster" && faults+=("hub /list unreadable")
grep -q 'roster=arboslife' <<<"$roster" || faults+=("arboslife not on the roster")
for need in demo feedback phone subnet120; do grep -q "roster=arboslife.*projects=[^ ]*\b$need\b" <<<"$roster" || faults+=("$need not registered"); done
grep -q 'roster=arboslife.*worker=True' <<<"$roster" || faults+=("worker daemon not registered")
gone_roster=$(grep -o 'gone=[^ ]*' <<<"$roster" | grep -v 'gone=-' | cut -d= -f2)
[ -n "$gone_roster" ] && faults+=("roster says binary_gone: $gone_roster")
gone_proc=$(grep '^GONE=' <<<"$remote" | cut -d= -f2- | tr '\n' ';')
[ -n "$gone_proc" ] && faults+=("process on a deleted image: $gone_proc")
double=$(grep '^DOUBLE=' <<<"$remote" | cut -d= -f2- | tr '\n' ';')
[ -n "$double" ] && faults+=("double-served place: $double")

# ---- the tunnel, from outside -------------------------------------------------
URL=$(op read "op://Arbos/6uihrhmgfwncp3jz3vxtfxklhi/arboslife-hub-url" 2>/dev/null | tr -d '\n' | sed 's#^wss://#https://#')
if [ -n "$URL" ]; then
  code=$(curl -s -m 12 -o /dev/null -w '%{http_code}' "$URL/healthz")
  [ "$code" = 200 ] || faults+=("hub tunnel $URL/healthz -> HTTP ${code:-none}")
else
  faults+=("could not read the hub URL from the vault")
fi

verdict=MESH-OK; [ ${#faults[@]} -gt 0 ] && verdict=MESH-FAULT
reason=$(IFS='; '; echo "${faults[*]}")
say "$verdict ${reason} | units=$nunits/6 healthz=$healthz tunnel=${code:-?} $(grep -o 'projects=[^ ]*' <<<"$roster") $(grep -o 'gone=[^ ]*' <<<"$roster")"

# ---- record --------------------------------------------------------------------
line=$(python3 -c 'import sys,json; a=sys.argv[1:]; print(json.dumps({"ts":a[0],"verdict":a[1],"reason":a[2],"units_active":int(a[3] or 0),"healthz":a[4],"tunnel_http":a[5],"roster":a[6][:300]},separators=(",",":")))' "$NOW" "$verdict" "$reason" "$nunits" "$healthz" "${code:-}" "$roster")
cd "$REPO" && git fetch -q origin "+refs/heads/$BRANCH:refs/remotes/origin/$BRANCH" 2>/dev/null
record() {
  local idx parent old blob tree commit; idx="$(mktemp -u)"
  parent="$(git rev-parse --verify -q "refs/remotes/origin/$BRANCH" || true)"; [ -n "$parent" ] || return 1
  old="$(git show "$parent:mesh/arboslife.jsonl" 2>/dev/null || true)"
  blob=$( { [ -n "$old" ] && printf '%s\n' "$old"; printf '%s\n' "$line"; } | git hash-object -w --stdin)
  GIT_INDEX_FILE="$idx" git read-tree "$parent" && GIT_INDEX_FILE="$idx" git update-index --add --cacheinfo "100644,$blob,mesh/arboslife.jsonl" \
    && tree=$(GIT_INDEX_FILE="$idx" git write-tree) && commit=$(git commit-tree "$tree" -p "$parent" -m "mesh-health $NOW $verdict") \
    && git push -q origin "$commit:refs/heads/$BRANCH"; local rc=$?; rm -f "$idx"; return $rc
}
record || { git fetch -q origin "+refs/heads/$BRANCH:refs/remotes/origin/$BRANCH"; record || say "could not record on $BRANCH"; }
echo "$verdict${reason:+ $reason}"
