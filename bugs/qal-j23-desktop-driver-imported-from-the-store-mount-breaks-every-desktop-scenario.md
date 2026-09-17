# qal-j23: the desktop scenarios import their driver from the store's FUSE mount, so a slow or faulting mount reads as 24 product breaks and silently takes the acceptance journey with it

- Measured at: cycle `20260917T134605Z` on `qa-vm2`, kernel `arbos-kernel 0.2.0 2f9a042707e1 protocol 1`, with `ARBOS_QA_DRIVER_DIR=/cursor/stores/bc-…/internal/parity` — which is what `deploy/vm-loop.sh` **prefers** whenever the store holds a copy of `arbosdriver.py`. Rollouts `internal/qa/rollouts/20260917T153857Z-journey-linux` and `…T154039Z-sq-02-desktop-stop-holds-follow-up`.
- Class: a rig fault that manufactures product breaks. Not a kernel bug — and that is the point: the breaks it files are indistinguishable from kernel faults in the cycle log, and one of them is the headline.
- Feature: `desktop_scenarios.py` (`DRIVER_DIR`, `available()`), `deploy/cycle.sh` step 3b2, `deploy/vm-loop.sh`'s default.

## What happens

`desktop_scenarios.py` puts `DRIVER_DIR` on `sys.path` and imports `arbosdriver` **once per scenario**. With
    10|`DRIVER_DIR` on the store's FUSE mount, every desktop scenario's import is a read of that mount. When the
mount is slow or faulting, the import raises, and the harness records it as a break:

| scenario | break | time |
|---|---|---|
| `journey-linux` — **the acceptance journey** | `driver-exception: OSError: [Errno 5] Input/output error: '…/internal/parity/arbosdriver.py'` | 10.1 s |
| `sq-02-desktop-stop-holds-follow-up` | `driver-exception: BlockingIOError: [Errno 11] Resource temporarily unavailable: '…/arbosdriver.py'` | **9142.7 s** (2.5 h, inside a `timeout 60m`) |
| 22 others tagged `desktop` | the same two errnos | — |

Twenty-four of the cycle's 38 breaks were this one cause, on a build with nothing wrong with it, and each
drafted a bug file. The journey scored `0/8 pass, 8 unverified` and the cycle log's journey line read as
    20|a real result.

Three things make it worse than "the mount is slow":

1. **`available()` cannot see it coming.** It tests `(Path(DRIVER_DIR) / "arbosdriver.py").exists()` — the
   file stats perfectly well. It is the *read* that fails. Present, and unreadable: the same distinction
   the kernel's own `record::read_text().confirmed()` exists to make (qal-j09's family), in the harness.
2. **The failure is scored against the product.** A `driver-exception` break names the scenario, so a
   cycle reports "the desktop stack broke 24 ways" when the desktop stack was never reached.
3. **`sq-02` held the step for 2.5 hours** inside a `timeout 60m`, so one faulting read also cost a cycle
   its remaining steps.

## What we expect
    30|
The driver is code the harness runs, not data under test, and it must come off local disk:

1. **`cycle.sh` copies the driver to `$ROOT/loop/driver` once per cycle** and points the scenarios there,
   saying how many files it copied — and shouting `!! DESKTOP DRIVER NOT COPIED` if it cannot, since the
   alternative is reading it from the mount on every import. Done.
2. `vm-loop.sh`'s default should stop preferring the store copy over the repo's `desktop/driver`. The store
   copy is the parity rig's living version, so it is the right *source*; it is the wrong thing to *import*.
3. A `driver-exception` should arguably not be a scenario break at all but a rig alarm, because no build
   can pass or fail it. Left as a recommendation: it changes what a red means, and the steward reads that.

## Regression check
    40|
The before-and-after is the cycle itself: 24 `driver-exception` breaks with the driver on the mount, and a
local copy that imports cleanly (`arbosdriver.__file__` under `$ROOT/loop/driver`). No probe stages a FUSE
I/O error on demand — and per the review list, a probe must fail the way the world fails, so inventing one
(a FIFO, a fuse with an injected error) would stand for something other than this. The property worth
asserting instead, and cheap: **no scenario's import path lies on the store**, checkable in one line at
registration time rather than by driving a fault.

## The same shape, twice in one day

`qal-j19`'s third shape was a reader that looked in the wrong place first; the mesh worker then found the
second-reader script had two homes and deleted the store copy so the branch is the only source. This is
    50|the third instance of the same family in our own tooling: a file with a store copy and a repo copy, where
the store copy is the one that gets used and the mount is the thing that fails. Worth a sweep of every
path the harness reads at run time for the same shape.
