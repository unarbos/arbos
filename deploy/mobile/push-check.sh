#!/bin/bash
# The push check against the hub's own report (#333): GET /push says enabled or why not;
# GET /push/test is 200 when Apple took a test alert to this token, 503 when push is off,
# 502 when Apple refused. Prints a one-line verdict; never prints the token.
export PATH="/opt/homebrew/bin:$PATH"
T=$(plutil -extract hubToken raw -o - ~/arbos/ios/Arbos/Secrets.plist); H=$(plutil -extract hubURL raw -o - ~/arbos/ios/Arbos/Secrets.plist)
BASE=$(echo "$H" | sed -E 's#^wss://#https://#; s#^ws://#http://#; s#/+$##')
S=$(curl -s -o /tmp/push.json -w '%{http_code}' -H "Authorization: Bearer $T" "$BASE/push")
if [ "$S" != "200" ]; then echo "PUSH status: GET /push -> HTTP $S (hub predates #333?)"; head -c 300 /tmp/push.json; echo; exit 2; fi
python3 - <<'PY'
import json; d=json.load(open('/tmp/push.json'))
en=d.get("enabled"); why=d.get("reason"); dev=d.get("devices",[]); att=d.get("attempts",[])
print(f"PUSH status: enabled={en} reason={why!r} topic={d.get('topic')} key_id={d.get('key_id')} devices={len(dev)} attempts={len(att)}")
for x in dev[:5]: print("  device", x)
for a in att[-3:]: print("  attempt", a)
PY
S=$(curl -s -o /tmp/pushtest.json -w '%{http_code}' -H "Authorization: Bearer $T" "$BASE/push/test")
echo "PUSH test: GET /push/test -> HTTP $S :: $(head -c 240 /tmp/pushtest.json)"
case "$S" in 200) echo "PUSH verdict: PASS — Apple took a test alert; the phone should have buzzed";; 503) echo "PUSH verdict: OFF (expected until the key exists) — reason above";; 502) echo "PUSH verdict: FAIL — Apple refused; see attempts";; *) echo "PUSH verdict: UNKNOWN";; esac
