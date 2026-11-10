# Chat resource ownership

Image bitmaps and remembered layout sizes are keyed by the captured account lifetime,
blob slot and ref. An account re-adoption gets a new receipt; a refresh within the same
account keeps its receipt. The 48 MiB bitmap budget, decode/trim/animation path and
cost formula are unchanged. Consumers must still reject retired contexts before lookup
or publication; a key does not revoke access by itself.

A recording permission request owns one consumable WorkLifetime receipt. Stop, cancel,
replacement and a retired caller invalidate it before a delayed callback can start audio
or publish denial. Captures own UUID temp paths; failed starts and completed/discarded
captures clean only their own file. The display filename, encoding, 16 kHz mono format,
level formula, timer and one-second threshold stay unchanged.

The retained checks compile actual methods and WorkLifetime with small collaborators.
Cache checks use Foundation NSCache with a UIImage identity/size collaborator (16 cases;
original 10 failures/6 controls, after16 pass). Recorder checks compile the complete
MemoRecorder with fake AV session/recorder/timer and synthetic temporary bytes (23 cases;
original16 failures/7 controls, after23 pass). Its fixture fixes the old private filename
clock to expose collisions and observes callback completion with an injected defer.
The earlier15 permission cases overlap these23 and are not added to the count.
These checks do not exercise UIKit decoding, a real microphone, a physical device or
SwiftUI rendering. The final full SDK/CI gates cover compilation and ordinary suites.

Run from repository root, using a fresh output path:

```sh
python3 -I -B .improve/state-machine-checks/ios-chat-resources/cache/check_cache.py --source-root . --out /tmp/arbos-cache-check
python3 -I -B .improve/state-machine-checks/ios-chat-resources/recorder/check_recorder.py --source-root . --out /tmp/arbos-recorder-check
```

Both binaries run with all network denied, fresh HOME and owned synthetic temp files.
PARITY.json pins the checked source and retained physical calibration.
