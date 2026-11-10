# Terminal room callback ownership

A403/404 socket failure is terminal. It now advances the same generation already
used by stop/reconnect before cancellation, so queued callbacks from that socket
cannot reopen core state or restart presence. Existing core RoomSession protocol,
started+stopped legal terminal shape, backoff, timers and physical calibration are
unchanged.

From the repository root on macOS with Python3 and swiftc:

```sh
python3 .improve/state-machine-checks/session-room/check.py --output /tmp/arbos-room-callback-1
```

The output directory must be fresh. The checker compiles actual connect/open/
receive/stop/tick/cancel method bodies, substitutes only local socket/core-action
observers, hashes the input before/after, denies network and uses a fresh temporary
build. It tests queued terminal403/404 callbacks plus current-open, explicit-stop
and reconnect controls. Baseline3PASS/2FAIL1.487s; candidate5PASS/.912s, no skipped
checks. Compiler errors are not behavior RED. `GATES.json` pins each source/stage.

A real XCTest regression for both statuses is retained in SessionRoomTests.swift;
the original tests are preserved. Swift syntax parse passed; full iOS SDK/XCTest
execution remains a separate platform gate. These checks do not exercise a real
URLSession transport, relay service or physical device.
