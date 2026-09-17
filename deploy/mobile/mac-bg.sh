#!/bin/bash
export PATH="/opt/homebrew/bin:$HOME/Library/Python/3.14/bin:$PATH"
U=B1185668-7488-420F-B12D-4412BAAC7673; O=$HOME/mobile-out/cycle-27; mkdir -p $O; B=com.unarbos.arbos.ios
T=$(plutil -extract hubToken raw -o - ~/arbos/ios/Arbos/Secrets.plist); H=$(plutil -extract hubURL raw -o - ~/arbos/ios/Arbos/Secrets.plist)
xcrun simctl terminate $U $B 2>/dev/null; sleep 1; xcrun simctl launch $U $B -hubURL "$H" -hubToken "$T" >/dev/null 2>&1; sleep 7
xcrun simctl io $U screenshot /tmp/l.png >/dev/null 2>&1; Y=$(python3 ~/find_row.py /tmp/l.png pod); idb ui tap 120 $Y --udid $U; sleep 5
idb ui tap 200 788 --udid $U; sleep 0.8; idb ui text "RESUME $(date -u +%H%M%S): reply with exactly: before the long sleep." --udid $U; idb ui key 40 --udid $U; sleep 8
xcrun simctl io $U screenshot $O/bg-01-before.png >/dev/null 2>&1
idb ui button HOME --udid $U; echo "$(date -u +%T) backgrounded"
sleep 720
xcrun simctl launch $U $B >/dev/null 2>&1; sleep 3; xcrun simctl io $U screenshot $O/bg-02-resumed.png >/dev/null 2>&1; sleep 4
idb ui tap 200 788 --udid $U; sleep 0.8; idb ui text "RESUME-AFTER: reply with exactly: after the long sleep." --udid $U; idb ui key 40 --udid $U; sleep 10
xcrun simctl io $U screenshot $O/bg-03-after-send.png >/dev/null 2>&1
python3 ~/kernel.py pod history 4 | cut -c1-100 > $O/bg-tail.txt; echo DONE >> $O/bg-tail.txt
