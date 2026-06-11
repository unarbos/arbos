#!/usr/bin/env bash
# Poll Bittensor Discord subnet 3 channel for new messages.
# Prints any messages newer than the last-seen id (oldest-first).
# Tracks state in scratch/bittensor_sn3_lastid so each run only emits new msgs.
set -euo pipefail

CHANNEL_ID="1493243812263362660"   # 3・teutonic・gamma  (Bittensor subnet 3)
STATE_DIR="$(cd "$(dirname "$0")/.." && pwd)/scratch"
STATE_FILE="$STATE_DIR/bittensor_sn3_lastid"
RESP_FILE="$STATE_DIR/bittensor_sn3_resp.json"
mkdir -p "$STATE_DIR"

if ! TOKEN="$(doppler secrets get DISCORD_BOT_TOKEN --project arbos --config dev --plain 2>/dev/null)" || [ -z "$TOKEN" ]; then
  echo "error: could not fetch DISCORD_BOT_TOKEN from doppler (project arbos, config dev) — check doppler auth" >&2
  exit 1
fi

LAST_ID=""
[ -f "$STATE_FILE" ] && LAST_ID="$(cat "$STATE_FILE")"

if [ -n "$LAST_ID" ]; then
  URL="https://discord.com/api/v10/channels/$CHANNEL_ID/messages?limit=100&after=$LAST_ID"
else
  URL="https://discord.com/api/v10/channels/$CHANNEL_ID/messages?limit=10"
fi

curl -s -H "Authorization: Bot $TOKEN" "$URL" -o "$RESP_FILE"

python3 - "$STATE_FILE" "$RESP_FILE" <<'PY'
import sys, json
state_file, resp_file = sys.argv[1], sys.argv[2]
try:
    with open(resp_file) as f:
        msgs = json.load(f)
except Exception as e:
    print("PARSE_ERROR", e); sys.exit(0)
if isinstance(msgs, dict):
    print("API_ERROR", msgs); sys.exit(0)
if not msgs:
    sys.exit(0)
msgs.sort(key=lambda m: int(m["id"]))  # oldest-first
for m in msgs:
    ts = m["timestamp"][11:19]
    author = m["author"].get("global_name") or m["author"]["username"]
    content = (m.get("content") or "").replace("\n", " ").strip()
    if not content and m.get("attachments"):
        content = "[attachment: %d file(s)]" % len(m["attachments"])
    if not content and m.get("embeds"):
        content = "[embed]"
    print(f"{ts} {author}: {content}")
with open(state_file, "w") as f:
    f.write(msgs[-1]["id"])
PY
