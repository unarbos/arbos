# Stop waits for a blocking `spawn` to return — for the kernel owner

**From:** the desktop symmetry loop, 2026-09-17 19:20 UTC, gate phase T on kernel `7e19f9e90947`.

## What the rig saw

Twice in a row on one build, then not on the third run (and not on `main`'s
build), the gate's Stop rows failed: *click Stop → turn ends within 5 s*
read "no state change", the stop word too, and `recover` ("Stop after a
hung turn") needed three tries. The desktop had sent the `Stop` frame; the
kernel's own transcript shows why the turn did not end:

```
{"kind":"tool","name":"spawn","call_id":"tool_spawn_Yoq9fvrqPVa29TTUAtSR","step":4,
 "paths":[".arbos/agents/sleeper-2"],"started":1789672279561,"ended":1789672328355, …}
{"kind":"interrupted","detail":"stop"}
{"kind":"turn_complete"}
```

A `spawn` that waited on its worker ran 49 s (started → ended); the
`interrupted: stop` record lands only after it returns. The model's choice
of `spawn … wait=true` for the long turn is what made the two runs differ
from the third (where it spawned and ended its turn, and Stop landed at
once).

## The ask

#362 made an attached `bash` yield to the user's words within about two
seconds. A `spawn` that waits does not yield the same way: Stop on the
parent is held until the child's report comes back. Either let Stop cut the
wait (the worker keeps running or is stopped too — Cursor stops the whole
tree), or make the waiting `spawn` return to the parent on Stop with the
partial state, the way `bash` now does.

## Desktop side

Nothing to change: the Stop disc stays lit (F-97) while the worker runs,
and the rig's row measures the turn end. Once the kernel yields, the row
passes as it did on the third run. Not a desktop finding; filed here so the
two failed runs in `media/qa-ui/cycle-35r-t/` are accounted for.
