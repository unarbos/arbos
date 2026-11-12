# Heavy/GUI lease laws

The retained test imports Effect 4 Schema/Arbitrary and asks a bounded persistent Python bridge for observations of the **actual pure function definitions** extracted from the three runner scripts. It does not source their effectful top levels, copy their transition algorithms, execute xcrun, inspect Shared lease files, or access a device. Python runs with `-I -B` and a PATH-only environment. Its Bash child has an empty PATH, restricted execution and disabled escape builtins; on macOS a sandbox denies network, writes, Shared reads and execution except Bash.

From a repository with the existing harness install:

```
node --test harness/arbos-pi-ext/lease-state-laws.test.ts
```

Run the retained test by absolute path from any cwd. Four fixed seeds (89001–89004), 256 cases each, check metadata and two owner observations before grant, absorbing abandonment/GUI failure, conservative capacity after invalid/failed retirement, and rejected/retired boot admission. Each law includes positive progress controls. Traces vary request mode, failure stage, invalid metadata, observation scope and late events. The oracle returns states, exit statuses, actual counter projections and actual boot-event construction, never verdict flags. A protocol test checks six invalid frames followed by successful recovery.

`LEASE_LAWS_GATES.json` records the final 5-test/1,024-case result, strict TypeScript validation and source hashes. `check_lease_mutants.py` makes temporary source copies, mutates actual `lease_counts` and `boot_lease_event`, and runs the same laws. Both mutations falsify the property, accept one canonical shrink, and fail exact replay; see the retained compact gate record. No controller source is modified by the negative controls.

These laws cover pure observations and transitions. Actual locking, deferred signals, atomic rename, renewal rollback, metadata files, owner liveness and adapter wiring are covered separately by the author's 34-case runner regression suite. The laws do not prove cross-user trust or a hardware scheduling policy beyond those existing adapters.

Portable negative control: `python3 .improve/state-machine-checks/check_lease_mutants.py --root . --out /tmp/arbos-lease-mutants-fresh` (choose a new output directory). The adapter copies only the three controllers and pure bridge, reruns the unmodified suite, and writes fresh isolated mutation/replay evidence. Root execution passed the 1,024-case control and detected/replayed both mutants.
